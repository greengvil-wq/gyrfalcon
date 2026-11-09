//! Адресные связи запросов, полей и явных прав для навигации.
use quick_xml::{events::Event, Reader};
use rayon::prelude::*;
use regex::Regex;
use rusqlite::{params, Connection};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

const TYPES: &[(&str, &str, &str)] = &[
    ("Catalogs", "Catalog", "справочник"),
    ("Documents", "Document", "документ"),
    (
        "InformationRegisters",
        "InformationRegister",
        "регистрсведений",
    ),
    (
        "AccumulationRegisters",
        "AccumulationRegister",
        "регистрнакопления",
    ),
    (
        "AccountingRegisters",
        "AccountingRegister",
        "регистрбухгалтерии",
    ),
    (
        "CalculationRegisters",
        "CalculationRegister",
        "регистррасчета",
    ),
    (
        "ChartsOfCharacteristicTypes",
        "ChartOfCharacteristicTypes",
        "планвидовхарактеристик",
    ),
    ("ChartsOfAccounts", "ChartOfAccounts", "плансчетов"),
    (
        "ChartsOfCalculationTypes",
        "ChartOfCalculationTypes",
        "планвидоврасчета",
    ),
    ("ExchangePlans", "ExchangePlan", "планобмена"),
    ("BusinessProcesses", "BusinessProcess", "бизнеспроцесс"),
    ("Tasks", "Task", "задача"),
    ("Constants", "Constant", "константа"),
    ("DataProcessors", "DataProcessor", "обработка"),
    ("Reports", "Report", "отчет"),
    ("Enums", "Enum", "перечисление"),
];
static TOKENS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}_][\p{L}\p{N}_]*|[().,]").unwrap());
static FIELDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([\p{L}_][\p{L}\p{N}_]*)\s*\.\s*([\p{L}_][\p{L}\p{N}_]*(?:\s*\.\s*[\p{L}_][\p{L}\p{N}_]*)*)").unwrap()
});

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS navigation_usages(id INTEGER PRIMARY KEY, object_ref TEXT NOT NULL, object_key TEXT NOT NULL, field TEXT NOT NULL, field_key TEXT NOT NULL, kind TEXT NOT NULL, file TEXT NOT NULL, line INTEGER NOT NULL, evidence TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_nav_object ON navigation_usages(object_key,field_key);
CREATE INDEX IF NOT EXISTS idx_nav_file ON navigation_usages(file);
CREATE TABLE IF NOT EXISTS navigation_rights(id INTEGER PRIMARY KEY, role TEXT NOT NULL, object_ref TEXT NOT NULL, object_key TEXT NOT NULL, right_name TEXT NOT NULL, allowed INTEGER NOT NULL, file TEXT NOT NULL, line INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS idx_nav_right ON navigation_rights(object_key);";

#[derive(Clone, Debug)]
struct Usage {
    target: String,
    field: String,
    kind: &'static str,
    file: String,
    line: usize,
    evidence: String,
    projection: Option<String>,
}
struct Right {
    role: String,
    target: String,
    right: String,
    allowed: bool,
    file: String,
    line: usize,
}
#[derive(Default)]
struct Context {
    objects: HashMap<String, String>,
    references: HashMap<(String, String), String>,
    forms: HashMap<String, HashMap<String, String>>,
}

fn context(conn: &Connection) -> rusqlite::Result<Context> {
    let mut ctx = Context::default();
    let mut st=conn.prepare("SELECT category,object_name FROM object_synonyms UNION SELECT category,object_name FROM modules WHERE object_name IS NOT NULL")?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (cat, name) = row?;
        if let Some((_, typ, _)) = TYPES.iter().find(|t| t.0 == cat) {
            let key = format!("{typ}.{name}");
            ctx.objects.insert(key.to_lowercase(), key);
        }
    }
    let mut st = conn.prepare(
        "SELECT file,element_name,element_type FROM form_elements WHERE kind='attribute'",
    )?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (file, name, types) = row?;
        let targets: HashSet<_> = types
            .split(", ")
            .filter_map(|t| normalize_type(t, &ctx))
            .collect();
        if targets.len() == 1 {
            ctx.forms
                .entry(file)
                .or_default()
                .insert(name.to_lowercase(), targets.into_iter().next().unwrap());
        }
    }
    let mut st=conn.prepare("SELECT category,object_name,attr_name,COALESCE(ts_name,''),attr_type FROM object_attributes")?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })? {
        let (cat, obj, name, tab, types) = row?;
        let Some((_, typ, _)) = TYPES.iter().find(|t| t.0 == cat) else {
            continue;
        };
        let types = types.unwrap_or_default();
        let types: Vec<String> = serde_json::from_str(&types).unwrap_or_else(|_| vec![types]);
        let targets: HashSet<_> = types
            .iter()
            .filter_map(|t| normalize_type(t, &ctx))
            .collect();
        if targets.len() == 1 {
            let field = if tab.is_empty() {
                name
            } else {
                format!("{tab}.{name}")
            };
            ctx.references.insert(
                (format!("{typ}.{obj}").to_lowercase(), field.to_lowercase()),
                targets.into_iter().next().unwrap(),
            );
        }
    }
    Ok(ctx)
}
fn normalize_type(t: &str, ctx: &Context) -> Option<String> {
    let (kind, name) = t.strip_prefix("cfg:").unwrap_or(t).split_once('.')?;
    let kind = kind
        .strip_suffix("ValueManager")
        .or_else(|| kind.strip_suffix("RecordSet"))
        .or_else(|| kind.strip_suffix("Object"))
        .or_else(|| kind.strip_suffix("Ref"))
        .or_else(|| kind.strip_suffix("Manager"))
        .unwrap_or(kind);
    ctx.objects
        .get(&format!("{kind}.{name}").to_lowercase())
        .cloned()
}

/// Переход по однозначным ссылочным реквизитам; составной тип не угадывается.
fn resolve_member(target: &str, field: &str, ctx: &Context) -> (String, String) {
    let mut target = target.to_string();
    let mut parts: Vec<&str> = field.split('.').collect();
    for _ in 0..32 {
        if parts.len() < 2 {
            break;
        }
        let mut next = None;
        for i in 1..parts.len() {
            let prefix = parts[..i].join(".").to_lowercase();
            if let Some(typ) = ctx.references.get(&(target.to_lowercase(), prefix)) {
                next = Some((typ.clone(), i));
                break;
            }
        }
        if let Some((typ, count)) = next {
            target = typ;
            parts.drain(..count);
        } else if parts[0].eq_ignore_ascii_case("Ссылка")
            && matches!(
                target.split('.').next(),
                Some(
                    "Catalog"
                        | "Document"
                        | "Enum"
                        | "ChartOfCharacteristicTypes"
                        | "ChartOfAccounts"
                        | "ChartOfCalculationTypes"
                        | "ExchangePlan"
                        | "BusinessProcess"
                        | "Task"
                )
            )
        {
            parts.remove(0);
        } else {
            break;
        }
    }
    (target, parts.join("."))
}

/// Комментарии и литералы маскируются с сохранением байтов и переносов.
fn lexical(source: &str) -> (String, Vec<(usize, String)>) {
    let bytes = source.as_bytes();
    let mut masked = bytes.to_vec();
    let mut strings = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                masked[i] = b' ';
                i += 1;
            }
        } else if bytes[i] == b'"' {
            let start = i;
            i += 1;
            let mut content = Vec::new();
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    if bytes.get(i + 1) == Some(&b'"') {
                        content.push(b'"');
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    content.push(bytes[i]);
                    i += 1;
                }
            }
            for b in &mut masked[start..i] {
                if *b != b'\n' && *b != b'\r' {
                    *b = b' ';
                }
            }
            strings.push((start, String::from_utf8_lossy(&content).into_owned()));
        } else {
            i += 1;
        }
    }
    (String::from_utf8(masked).unwrap(), strings)
}
fn emit(
    out: &mut Vec<Usage>,
    target: &str,
    field: &str,
    kind: &'static str,
    file: &str,
    line: usize,
    evidence: &str,
) {
    out.push(Usage {
        target: target.into(),
        field: field.into(),
        kind,
        file: file.into(),
        line,
        evidence: evidence.trim().chars().take(240).collect(),
        projection: None,
    });
}
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        )
        .collect()
}
fn line_number(starts: &[usize], pos: usize) -> usize {
    starts.partition_point(|p| *p <= pos)
}
fn query(
    text: &str,
    base: usize,
    file: &str,
    kind: &'static str,
    ctx: &Context,
    out: &mut Vec<Usage>,
) {
    let starts = line_starts(text);
    let text_lines: Vec<&str> = text.lines().collect();
    let low = text.to_lowercase();
    if !low.contains("выбрать") && !low.contains("select") {
        return;
    }
    let (code, _) = lexical(text);
    let tokens: Vec<_> = TOKENS
        .find_iter(&code)
        .map(|m| (m.as_str(), m.start()))
        .collect();
    if !tokens.first().is_some_and(|t| {
        ["выбрать", "select", "уничтожить", "drop"].contains(&t.0.to_lowercase().as_str())
    }) {
        return;
    }
    // Ссылка на тип или значение в запросе не означает чтение таблицы.
    for (offset, line) in code.lines().enumerate() {
        let mut refs = Vec::new();
        gyrfalcon_parser::usages::упоминания_в_литерале(
            line,
            (base + offset) as u32,
            &mut refs,
        );
        for reference in refs {
            if let Some(target) = ctx.objects.get(&reference.object_ref_key) {
                emit(
                    out,
                    target,
                    "",
                    if kind == "bsl_query" {
                        "bsl_query_reference"
                    } else {
                        "xml_query_reference"
                    },
                    file,
                    base + offset,
                    text_lines.get(offset).copied().unwrap_or(""),
                );
            }
        }
    }
    let mut aliases: HashMap<String, HashSet<String>> = HashMap::new();
    for i in 0..tokens.len().saturating_sub(3) {
        if !["из", "соединение", "from", "join"].contains(&tokens[i].0.to_lowercase().as_str())
            || tokens[i + 2].0 != "."
        {
            continue;
        }
        let Some((_, typ, _)) = TYPES.iter().find(|t| {
            t.2 == tokens[i + 1].0.to_lowercase()
                || t.1.to_lowercase() == tokens[i + 1].0.to_lowercase()
        }) else {
            continue;
        };
        let Some(target) = ctx
            .objects
            .get(&format!("{typ}.{}", tokens[i + 3].0).to_lowercase())
        else {
            continue;
        };
        let line = base
            + text[..tokens[i + 1].1]
                .bytes()
                .filter(|b| *b == b'\n')
                .count();
        emit(
            out,
            target,
            "",
            kind,
            file,
            line,
            text_lines.get(line - base).copied().unwrap_or(""),
        );
        let mut j = i + 4;
        if tokens.get(j).is_some_and(|t| t.0 == ".") {
            j += 2;
        }
        if tokens.get(j).is_some_and(|t| t.0 == "(") {
            let mut depth = 0;
            while j < tokens.len() {
                if tokens[j].0 == "(" {
                    depth += 1;
                }
                if tokens[j].0 == ")" {
                    depth -= 1;
                    if depth == 0 {
                        j += 1;
                        break;
                    }
                }
                j += 1;
            }
        }
        let alias = if tokens
            .get(j)
            .is_some_and(|t| ["как", "as"].contains(&t.0.to_lowercase().as_str()))
        {
            tokens.get(j + 1).map(|t| t.0).unwrap_or(tokens[i + 3].0)
        } else {
            tokens[i + 3].0
        };
        aliases
            .entry(alias.to_lowercase())
            .or_default()
            .insert(target.clone());
    }
    let projection_end = tokens
        .iter()
        .find(|t| ["из", "from"].contains(&t.0.to_lowercase().as_str()))
        .map(|t| t.1)
        .unwrap_or(0);
    for cap in FIELDS.captures_iter(&code) {
        let Some(targets) = aliases.get(&cap[1].to_lowercase()) else {
            continue;
        };
        if targets.len() != 1 {
            continue;
        }
        let target = targets.iter().next().unwrap();
        let field: String = cap[2].chars().filter(|c| !c.is_whitespace()).collect();
        let pos = cap.get(0).unwrap().start();
        let line = base + line_number(&starts, pos) - 1;
        emit(
            out,
            target,
            &field,
            kind,
            file,
            line,
            text_lines.get(line - base).copied().unwrap_or(""),
        );
        let (resolved, member) = resolve_member(target, &field, ctx);
        if resolved != *target || member != field {
            emit(
                out,
                &resolved,
                &member,
                kind,
                file,
                line,
                text_lines.get(line - base).copied().unwrap_or(""),
            );
        }
        if pos < projection_end {
            let before = TOKENS
                .find_iter(&code[..pos])
                .last()
                .map(|t| t.as_str().to_lowercase())
                .unwrap_or_default();
            let tail = &code[cap.get(0).unwrap().end()..];
            let after = tail.trim_start_matches(|c: char| c.is_whitespace() || c == '|');
            let word = TOKENS
                .find(after)
                .map(|t| t.as_str().to_lowercase())
                .unwrap_or_default();
            if ![
                "выбрать",
                "select",
                ",",
                "различные",
                "distinct",
                "разрешенные",
            ]
            .contains(&before.as_str())
                || !(after.starts_with(',')
                    || ["как", "as", "из", "from"].contains(&word.as_str())
                        && after.chars().next().is_some_and(|c| c.is_alphabetic()))
            {
                continue;
            }

            let mut words = TOKENS.find_iter(tail);
            let alias = if words
                .next()
                .is_some_and(|w| ["как", "as"].contains(&w.as_str().to_lowercase().as_str()))
            {
                words.next().map(|w| w.as_str().to_string())
            } else {
                None
            };
            if let Some(row) = out.last_mut() {
                row.projection = Some(
                    alias.unwrap_or_else(|| field.rsplit('.').next().unwrap_or(&field).to_string()),
                );
            }
        }
    }
}
fn field_paths(
    text: &str,
    bindings: &HashMap<String, String>,
    base: usize,
    file: &str,
    kind: &'static str,
    out: &mut Vec<Usage>,
) {
    let starts = line_starts(text);
    let text_lines: Vec<&str> = text.lines().collect();
    for cap in FIELDS.captures_iter(text) {
        if let Some(target) = bindings.get(&cap[1].to_lowercase()) {
            let line = base + line_number(&starts, cap.get(0).unwrap().start()) - 1;
            let mut field: String = cap[2].chars().filter(|c| !c.is_whitespace()).collect();
            if text[cap.get(0).unwrap().end()..]
                .trim_start()
                .starts_with('(')
            {
                if let Some((prefix, _)) = field.rsplit_once('.') {
                    field = prefix.to_string();
                } else {
                    continue;
                }
            }
            emit(
                out,
                target,
                &field,
                kind,
                file,
                line,
                text_lines.get(line - base).copied().unwrap_or(""),
            );
        }
    }
}
fn scan(path: &Path, src: &Path, ctx: &Context) -> Result<(Vec<Usage>, Vec<Right>), String> {
    let file = path
        .strip_prefix(src)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let source = std::fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
    let source = source.trim_start_matches('\u{feff}');
    let source_lines = line_starts(source);
    let mut usages = Vec::new();
    let mut rights = Vec::new();
    if path.extension().is_some_and(|s| s == "bsl") {
        let (code, literals) = lexical(source);
        for (start, text) in literals {
            query(
                &text,
                line_number(&source_lines, start),
                &file,
                "bsl_query",
                ctx,
                &mut usages,
            );
        }
        let form = file.replace("/Form/Module.bsl", "/Form.xml");
        let mut bindings = ctx.forms.get(&form).cloned().unwrap_or_default();
        if file.ends_with("/ObjectModule.bsl") || file.ends_with("/RecordSetModule.bsl") {
            let parts: Vec<_> = file.split('/').collect();
            if let Some((_, typ, _)) = TYPES.iter().find(|t| Some(&t.0) == parts.first()) {
                if let Some(target) = parts
                    .get(1)
                    .and_then(|name| ctx.objects.get(&format!("{typ}.{name}").to_lowercase()))
                {
                    bindings.insert("этотобъект".into(), target.clone());
                    bindings.insert("thisobject".into(), target.clone());
                }
            }
        }
        field_paths(&code, &bindings, 1, &file, "code_field", &mut usages);
    } else {
        let mut reader = Reader::from_str(source);
        let mut tags: Vec<String> = Vec::new();
        let mut object = String::new();
        let mut right = String::new();
        let mut attribute: Vec<(usize, String)> = Vec::new();
        let mut dynamic = HashSet::new();
        let mut manual: HashMap<String, bool> = HashMap::new();
        let mut direct_lists = HashMap::new();
        let mut projections: HashMap<(String, String), HashSet<(String, String)>> = HashMap::new();
        let mut pending = Vec::new();
        let lines: Vec<usize> = source
            .bytes()
            .enumerate()
            .filter_map(|(i, b)| (b == b'\n').then_some(i))
            .collect();
        loop {
            let pos = reader.buffer_position() as usize;
            let line = 1 + lines.partition_point(|p| *p < pos);
            match reader.read_event().map_err(|e| format!("{file}: {e}"))? {
                Event::Start(e) => {
                    let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    if tag == "Attribute" {
                        if let Some(name) =
                            e.attributes().flatten().find(|a| a.key.as_ref() == b"name")
                        {
                            attribute.push((
                                tags.len() + 1,
                                String::from_utf8_lossy(name.value.as_ref()).into_owned(),
                            ));
                        }
                    }
                    if tag == "right" {
                        right.clear();
                    }
                    tags.push(tag);
                }
                Event::End(_) => {
                    if tags.last().is_some_and(|t| t == "object") {
                        object.clear();
                    }
                    if attribute.last().is_some_and(|a| a.0 == tags.len()) {
                        attribute.pop();
                    }
                    tags.pop();
                }
                Event::Text(t) => {
                    let text = t.unescape().map_err(|e| e.to_string())?;
                    if tags.last().is_some_and(|t| {
                        t.eq_ignore_ascii_case("query") || t.eq_ignore_ascii_case("queryText")
                    }) {
                        let start = usages.len();
                        query(&text, line, &file, "xml_query", ctx, &mut usages);
                        let owner = attribute
                            .last()
                            .map(|a| a.1.to_lowercase())
                            .unwrap_or_default();
                        for row in &usages[start..] {
                            if let Some(alias) = &row.projection {
                                projections
                                    .entry((owner.clone(), alias.to_lowercase()))
                                    .or_default()
                                    .insert((row.target.clone(), row.field.clone()));
                            }
                        }
                    }
                    let owner = attribute
                        .last()
                        .map(|a| a.1.to_lowercase())
                        .unwrap_or_default();
                    if tags.last().is_some_and(|t| t == "ManualQuery") {
                        manual.insert(owner.clone(), text.trim() == "true");
                    }
                    if tags.last().is_some_and(|t| t == "MainTable") {
                        if let Some(target) = ctx.objects.get(&text.trim().to_lowercase()) {
                            emit(
                                &mut usages,
                                target,
                                "",
                                "xml_table_source",
                                &file,
                                line,
                                &text,
                            );
                            if manual.get(&owner) == Some(&false) {
                                direct_lists.insert(owner, target.clone());
                            }
                        }
                    }
                    if tags.last().is_some_and(|t| t == "Type") && text.trim() == "cfg:DynamicList"
                    {
                        if let Some((_, name)) = attribute.last() {
                            dynamic.insert(name.to_lowercase());
                        }
                    }
                    if tags.last().is_some_and(|t| {
                        t.eq_ignore_ascii_case("dataPath") || t.eq_ignore_ascii_case("field")
                    }) {
                        pending.push((
                            text.trim().trim_start_matches('~').to_string(),
                            line,
                            attribute
                                .last()
                                .map(|a| a.1.to_lowercase())
                                .unwrap_or_default(),
                        ));
                    }
                    if tags.last().is_some_and(|t| t == "DataPath") {
                        if let Some(bindings) = ctx.forms.get(&file) {
                            field_paths(&text, bindings, line, &file, "form_path", &mut usages);
                        }
                    }
                    let parent = tags.iter().rev().nth(1).map(String::as_str);
                    if file.starts_with("Roles/") && file.ends_with("/Rights.xml") {
                        match (tags.last().map(String::as_str), parent) {
                            (Some("name"), Some("object")) => object = text.trim().into(),
                            (Some("name"), Some("right")) => right = text.trim().into(),
                            (Some("value"), Some("right"))
                                if ["true", "false"].contains(&text.trim())
                                    && !object.is_empty()
                                    && !right.is_empty() =>
                            {
                                rights.push(Right {
                                    role: file.split('/').nth(1).unwrap_or("").into(),
                                    target: object.clone(),
                                    right: right.clone(),
                                    allowed: text.trim() == "true",
                                    file: file.clone(),
                                    line,
                                })
                            }
                            _ => {}
                        }
                    }
                }
                Event::CData(t) => query(
                    &String::from_utf8_lossy(t.as_ref()),
                    line,
                    &file,
                    "xml_query",
                    ctx,
                    &mut usages,
                ),
                Event::Eof => break,
                _ => {}
            }
        }
        for (path, line, owner) in pending {
            let parts: Vec<&str> = path.split('.').collect();
            let (scope, output, rest) =
                if parts.len() > 1 && dynamic.contains(&parts[0].to_lowercase()) {
                    (
                        parts[0].to_lowercase(),
                        parts[1].to_lowercase(),
                        parts[2..].join("."),
                    )
                } else if parts.len() == 1 {
                    (owner, parts[0].to_lowercase(), String::new())
                } else {
                    continue;
                };
            if !projections.contains_key(&(scope.clone(), output.clone())) {
                if let Some(target) = direct_lists.get(&scope) {
                    let field = if rest.is_empty() {
                        output.clone()
                    } else {
                        format!("{output}.{rest}")
                    };
                    let (target, field) = resolve_member(target, &field, ctx);
                    emit(
                        &mut usages,
                        &target,
                        &field,
                        "form_path",
                        &file,
                        line,
                        &path,
                    );
                }
            }
            if let Some(targets) = projections.get(&(scope, output)) {
                if targets.len() == 1 {
                    let (target, field) = targets.iter().next().unwrap();
                    let field = if rest.is_empty() {
                        field.clone()
                    } else {
                        format!("{field}.{rest}")
                    };
                    let (target, field) = resolve_member(target, &field, ctx);
                    emit(
                        &mut usages,
                        &target,
                        &field,
                        "form_path",
                        &file,
                        line,
                        &path,
                    );
                }
            }
        }
    }
    let derived: Vec<_> = usages
        .iter()
        .filter(|r| r.field.contains('.'))
        .filter_map(|r| {
            let (target, field) = resolve_member(&r.target, &r.field, ctx);
            (target != r.target || field != r.field).then(|| Usage {
                target,
                field,
                ..r.clone()
            })
        })
        .collect();
    usages.extend(derived);
    Ok((usages, rights))
}
fn write(conn: &Connection, rows: Vec<(Vec<Usage>, Vec<Right>)>) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let mut seen = HashSet::new();
    for (usages, rights) in rows {
        for u in usages {
            if !seen.insert((
                u.target.clone(),
                u.field.clone(),
                u.kind,
                u.file.clone(),
                u.line,
            )) {
                continue;
            }
            tx.execute("INSERT INTO navigation_usages(object_ref,object_key,field,field_key,kind,file,line,evidence) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![u.target,u.target.to_lowercase(),u.field,u.field.to_lowercase(),u.kind,u.file,u.line,u.evidence])?;
        }
        for r in rights {
            tx.execute("INSERT INTO navigation_rights(role,object_ref,object_key,right_name,allowed,file,line) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![r.role,r.target,r.target.to_lowercase(),r.right,r.allowed,r.file,r.line])?;
        }
    }
    tx.commit()
}
pub fn build(conn: &Connection, src: &Path) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let ctx = context(conn).map_err(|e| e.to_string())?;
    let paths: Vec<PathBuf> = walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|s| s == "bsl" || s == "xml")
        })
        .map(|e| e.into_path())
        .collect();
    let rows: Result<Vec<_>, _> = paths.par_iter().map(|p| scan(p, src, &ctx)).collect();
    write(conn, rows?).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO index_meta(key,value) VALUES('navigation_state','ready')",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn refresh(conn: &Connection, src: &Path, files: &[PathBuf]) -> Result<(), String> {
    if conn
        .prepare("SELECT 1 FROM navigation_usages LIMIT 0")
        .is_err()
    {
        return Ok(());
    }
    if files
        .iter()
        .any(|p| p.extension().is_none_or(|e| e != "bsl"))
    {
        return Err("адресные связи XML требуют полной пересборки".into());
    }
    let ctx = context(conn).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO index_meta(key,value) VALUES('navigation_state','stale')",
        [],
    )
    .map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for path in files {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            src.join(path)
        };
        let rel = path
            .strip_prefix(src)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        conn.execute("DELETE FROM navigation_usages WHERE file=?1", [&rel])
            .map_err(|e| e.to_string())?;
        if path.is_file() {
            rows.push(scan(&path, src, &ctx)?);
        }
    }
    write(conn, rows).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO index_meta(key,value) VALUES('navigation_state','ready')",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx() -> Context {
        Context {
            objects: HashMap::from([("catalog.товары".into(), "Catalog.Товары".into())]),
            ..Default::default()
        }
    }
    #[test]
    fn ссылка_в_запросе_и_простая_проекция() {
        let mut c = ctx();
        c.objects.insert(
            "informationregister.связи".into(),
            "InformationRegister.Связи".into(),
        );
        c.references.insert(
            ("informationregister.связи".into(), "документ".into()),
            "Catalog.Товары".into(),
        );
        let mut rows = Vec::new();
        query(
            "ВЫБРАТЬ Р.Документ.Код КАК Вывод ИЗ РегистрСведений.Связи КАК Р",
            1,
            "f",
            "xml_query",
            &c,
            &mut rows,
        );
        assert!(rows.iter().any(|r| r.target == "Catalog.Товары"
            && r.field == "Код"
            && r.projection.as_deref() == Some("Вывод")));
        let mut aggregate = Vec::new();
        query(
            "ВЫБРАТЬ СУММА(Р.Количество) КАК Итого ИЗ РегистрСведений.Связи КАК Р",
            1,
            "f",
            "xml_query",
            &c,
            &mut aggregate,
        );
        assert!(aggregate.iter().all(|r| r.projection.is_none()));
    }

    #[test]
    fn запрос_псевдоним_и_ложные_совпадения() {
        let mut rows = Vec::new();
        query(
            "ВЫБРАТЬ Т.Код ИЗ Справочник.Товары КАК Т ГДЕ Т.Код = &Код",
            10,
            "f",
            "bsl_query",
            &ctx(),
            &mut rows,
        );
        assert!(rows.iter().any(|r| r.field == "Код" && r.line == 10));
        assert_eq!(
            rows.iter()
                .filter(|r| r.field.is_empty() && r.kind == "bsl_query")
                .count(),
            1
        );
        let mut no = Vec::new();
        query(
            "ВЫБРАТЬ Т.Код ИЗ ВременнаяТаблица КАК Т",
            1,
            "f",
            "bsl_query",
            &ctx(),
            &mut no,
        );
        assert!(no.is_empty());
    }
    #[test]
    fn комментарии_и_литералы_не_поля() {
        let (masked, strings) = lexical("// Объект.Код\nА=\"Объект.Код\"; Объект.Имя;");
        let mut rows = Vec::new();
        field_paths(
            &masked,
            &HashMap::from([("объект".into(), "Catalog.Товары".into())]),
            1,
            "f",
            "code_field",
            &mut rows,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "Имя");
        assert_eq!(strings.len(), 1);
    }
    #[test]
    fn виртуальная_таблица_и_параметры() {
        let mut c = ctx();
        c.objects.insert(
            "informationregister.остатки".into(),
            "InformationRegister.Остатки".into(),
        );
        let mut rows = Vec::new();
        query("ВЫБРАТЬ Р.Цена ИЗ РегистрСведений.Остатки.СрезПоследних(&Дата, Вид=ЗНАЧЕНИЕ(Перечисление.Виды.Товар)) КАК Р",1,"f","xml_query",&c,&mut rows);
        assert!(rows.iter().any(|r| r.field == "Цена"));
    }
}
