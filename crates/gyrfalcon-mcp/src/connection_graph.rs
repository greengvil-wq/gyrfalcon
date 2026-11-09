//! Полная компонента связности индекса без ограничения числа узлов или рёбер.
//! Строки источников агрегируются по паре узлов и виду связи; число вхождений
//! сохраняется. Неразрешённые вызовы не превращаются в выдуманные зависимости.
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

#[derive(Clone)]
struct Node {
    key: String,
    kind: u8,
    name: String,
    category: String,
    file: Option<usize>,
    line: i64,
    owner: Option<usize>,
    field: String,
}
struct Edge {
    from: usize,
    to: usize,
    kind: usize,
    file: Option<usize>,
    line: i64,
    count: usize,
}
#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    keys: HashMap<String, usize>,
    edge_keys: HashMap<(usize, usize, usize), usize>,
    files: Vec<String>,
    file_keys: HashMap<String, usize>,
    kinds: Vec<Value>,
    kind_keys: HashMap<String, usize>,
    types: HashMap<String, String>,
    declared: Vec<usize>,
    methods: HashMap<String, usize>,
    modules: HashMap<String, usize>,
    intervals: HashMap<String, Vec<(i64, i64, usize)>>,
    forms: HashMap<(usize, String), usize>,
    subsystems: HashMap<String, usize>,
    warnings: Vec<String>,
}
const TYPES: &[(&str, &str)] = &[
    ("Configuration", "Configuration"),
    ("Catalog", "Catalogs"),
    ("Document", "Documents"),
    ("Enum", "Enums"),
    ("Report", "Reports"),
    ("DataProcessor", "DataProcessors"),
    ("InformationRegister", "InformationRegisters"),
    ("AccumulationRegister", "AccumulationRegisters"),
    ("AccountingRegister", "AccountingRegisters"),
    ("CalculationRegister", "CalculationRegisters"),
    ("ChartOfCharacteristicTypes", "ChartsOfCharacteristicTypes"),
    ("ChartOfAccounts", "ChartsOfAccounts"),
    ("ChartOfCalculationTypes", "ChartsOfCalculationTypes"),
    ("BusinessProcess", "BusinessProcesses"),
    ("Task", "Tasks"),
    ("ExchangePlan", "ExchangePlans"),
    ("Constant", "Constants"),
    ("DefinedType", "DefinedTypes"),
    ("Role", "Roles"),
    ("Subsystem", "Subsystems"),
    ("CommonModule", "CommonModules"),
    ("CommonForm", "CommonForms"),
    ("CommonAttribute", "CommonAttributes"),
    ("CommonCommand", "CommonCommands"),
    ("CommonTemplate", "CommonTemplates"),
    ("CommonPicture", "CommonPictures"),
    ("EventSubscription", "EventSubscriptions"),
    ("ScheduledJob", "ScheduledJobs"),
    ("FunctionalOption", "FunctionalOptions"),
    ("FunctionalOptionsParameter", "FunctionalOptionsParameters"),
    ("SettingsStorage", "SettingsStorages"),
    ("SessionParameter", "SessionParameters"),
    ("FilterCriterion", "FilterCriteria"),
    ("WebService", "WebServices"),
    ("HTTPService", "HTTPServices"),
    ("XDTOPackage", "XDTOPackages"),
    ("DocumentJournal", "DocumentJournals"),
    ("Language", "Languages"),
    ("CommandGroup", "CommandGroups"),
    ("WSReference", "WSReferences"),
    ("ExternalDataSource", "ExternalDataSources"),
    ("Style", "Styles"),
    ("StyleItem", "StyleItems"),
    ("Interface", "Interfaces"),
];
fn object_key(cat: &str, name: &str) -> String {
    format!("object:{cat}:{}", name.to_lowercase())
}
fn method_key(file: &str, name: &str) -> String {
    format!("method:{file}::{}", name.to_lowercase())
}
fn has(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get::<_, i64>(0),
    )
    .is_ok()
}
/// Читаем целиком, а не через пагинированные MCP-запросы. Ошибки не скрываются.
fn table(conn: &Connection, sql: &str) -> Result<Vec<Vec<Value>>, String> {
    let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
    let width = st.column_count();
    let rows = st
        .query_map([], |r| {
            let mut row = Vec::new();
            for i in 0..width {
                row.push(match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(n) => json!(n),
                    rusqlite::types::ValueRef::Text(s) => json!(String::from_utf8_lossy(s)),
                    _ => Value::Null,
                });
            }
            Ok(row)
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string())
}
fn str_at(row: &[Value], i: usize) -> &str {
    row[i].as_str().unwrap_or("")
}
fn num_at(row: &[Value], i: usize) -> i64 {
    row[i].as_i64().unwrap_or(0)
}
impl Graph {
    fn file(&mut self, file: &str) -> Option<usize> {
        if file.is_empty() {
            return None;
        }
        if let Some(id) = self.file_keys.get(file) {
            return Some(*id);
        }
        let id = self.files.len();
        self.files.push(file.into());
        self.file_keys.insert(file.into(), id);
        Some(id)
    }
    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        key: String,
        kind: u8,
        name: &str,
        cat: &str,
        file: &str,
        line: i64,
        owner: Option<usize>,
        field: &str,
    ) -> usize {
        let source = self.file(file);
        if let Some(id) = self.keys.get(&key).copied() {
            if self.nodes[id].file.is_none() {
                self.nodes[id].file = source;
            }
            if self.nodes[id].line == 0 {
                self.nodes[id].line = line;
            }
            return id;
        }
        let id = self.nodes.len();
        self.keys.insert(key.clone(), id);
        self.nodes.push(Node {
            key,
            kind,
            name: name.into(),
            category: cat.into(),
            file: source,
            line,
            owner,
            field: field.into(),
        });
        id
    }
    fn kind(&mut self, code: &str, label: &str, group: &str) -> usize {
        if let Some(id) = self.kind_keys.get(code) {
            return *id;
        }
        let id = self.kinds.len();
        self.kind_keys.insert(code.into(), id);
        self.kinds.push(json!([code, label, group]));
        id
    }
    fn edge(&mut self, a: usize, b: usize, kind: usize, file: &str, line: i64) {
        let source = self.file(file);
        if let Some(i) = self.edge_keys.get(&(a, b, kind)) {
            self.edges[*i].count += 1;
            return;
        }
        let id = self.edges.len();
        self.edge_keys.insert((a, b, kind), id);
        self.edges.push(Edge {
            from: a,
            to: b,
            kind,
            file: source,
            line,
            count: 1,
        });
    }
    fn object(&mut self, cat: &str, name: &str) -> usize {
        self.node(object_key(cat, name), 0, name, cat, "", 0, None, "")
    }
    fn field(&mut self, owner: usize, name: &str) -> usize {
        let parent = self.nodes[owner].clone();
        let key = format!("field:{}::{}", parent.key, name.to_lowercase());
        let file = parent
            .file
            .map(|i| self.files[i].clone())
            .unwrap_or_default();
        let id = self.node(key, 4, name, &parent.category, &file, 0, Some(owner), name);
        let k = self.kind("contains_field", "Содержит поле / значение", "metadata");
        if !self.edge_keys.contains_key(&(owner, id, k)) {
            self.edge(owner, id, k, "", 0);
        }
        id
    }
    fn form(&mut self, owner: usize, name: &str, file: &str) -> usize {
        if let Some(id) = self.forms.get(&(owner, name.into())) {
            return *id;
        }
        let key = if file.is_empty() {
            format!("form:{}::{name}", self.nodes[owner].key)
        } else {
            format!("form:{file}")
        };
        let cat = self.nodes[owner].category.clone();
        let id = self.node(key, 3, name, &cat, file, 0, Some(owner), "");
        self.forms.insert((owner, name.into()), id);
        let k = self.kind("contains_form", "Содержит форму", "forms");
        self.edge(owner, id, k, file, 0);
        id
    }
    fn member(&mut self, owner: usize, kind: u8, name: &str, file: &str) -> usize {
        let key = format!(
            "member:{}:{kind}:{}",
            self.nodes[owner].key,
            name.to_lowercase()
        );
        let cat = self.nodes[owner].category.clone();
        let id = self.node(key, kind, name, &cat, file, 0, Some(owner), "");
        let k = self.kind(
            "form_member",
            "Содержит элемент / реквизит / команду",
            "forms",
        );
        if !self.edge_keys.contains_key(&(owner, id, k)) {
            self.edge(owner, id, k, file, 0);
        }
        id
    }
    fn target(&mut self, reference: &str) -> Option<usize> {
        let raw = reference.strip_prefix("cfg:").unwrap_or(reference);
        let (typ, tail) = raw.split_once('.')?;
        let normal = typ
            .strip_suffix("ValueManager")
            .or_else(|| typ.strip_suffix("RecordSet"))
            .or_else(|| typ.strip_suffix("Object"))
            .or_else(|| typ.strip_suffix("Ref"))
            .or_else(|| typ.strip_suffix("Manager"))
            .unwrap_or(typ);
        let cat = self.types.get(&normal.to_lowercase())?.clone();
        if normal.eq_ignore_ascii_case("Subsystem") {
            let qualified = tail.replace(".Subsystem.", ".");
            return Some(
                self.subsystems
                    .get(&qualified.to_lowercase())
                    .copied()
                    .unwrap_or_else(|| self.object(&cat, &qualified)),
            );
        }
        if let Some((name, rest)) = tail.split_once(".TabularSection.") {
            let owner = self.object(&cat, name);
            let field = rest.replace(".Attribute.", ".");
            return Some(self.field(owner, &field));
        }
        if let Some((name, field)) = tail.split_once(".Attribute.") {
            let owner = self.object(&cat, name);
            return Some(self.field(owner, field.split('.').next().unwrap_or(field)));
        }
        for marker in [
            ".Dimension.",
            ".Resource.",
            ".EnumValue.",
            ".StandardAttribute.",
        ] {
            if let Some((name, field)) = tail.split_once(marker) {
                let owner = self.object(&cat, name);
                return Some(self.field(owner, field));
            }
        }
        if let Some((name, form)) = tail.split_once(".Form.") {
            let owner = self.object(&cat, name);
            return Some(self.form(owner, form, ""));
        }
        for marker in [".Command.", ".Template.", ".StandardCommand."] {
            if let Some((name, member)) = tail.split_once(marker) {
                let owner = self.object(&cat, name);
                return Some(self.field(owner, &format!("{}.{member}", marker.trim_matches('.'))));
            }
        }
        if let Some((name, _)) = tail.split_once('.') {
            let key = object_key(&cat, name);
            if let Some(id) = self.keys.get(&key) {
                return Some(*id);
            }
        }
        Some(self.object(&cat, tail))
    }
    fn type_targets(&mut self, reference: &str) -> Vec<usize> {
        let raw = reference.strip_prefix("cfg:").unwrap_or(reference);
        if raw.contains('.') {
            return self.target(raw).into_iter().collect();
        }
        let normal = raw
            .strip_suffix("ValueManager")
            .or_else(|| raw.strip_suffix("RecordSet"))
            .or_else(|| raw.strip_suffix("Object"))
            .or_else(|| raw.strip_suffix("Ref"))
            .or_else(|| raw.strip_suffix("Manager"))
            .unwrap_or(raw);
        let Some(cat) = self.types.get(&normal.to_lowercase()) else {
            return vec![];
        };
        self.declared
            .iter()
            .copied()
            .filter(|i| self.nodes[*i].category == *cat)
            .collect()
    }
    fn method(&self, module: &str, name: &str) -> Option<usize> {
        self.methods
            .get(&format!("{module}::{}", name.to_lowercase()))
            .copied()
    }
    fn handler(&self, module: &str, name: &str) -> Option<usize> {
        self.method(&format!("CommonModules/{module}/Ext/Module.bsl"), name)
    }
    fn source(&mut self, file: &str, line: i64) -> usize {
        if file.ends_with(".bsl") {
            if let Some(list) = self.intervals.get(file) {
                let i = list.partition_point(|r| r.0 <= line);
                if i > 0 && line <= list[i - 1].1 {
                    return list[i - 1].2;
                }
            }
            if let Some(id) = self.modules.get(file) {
                return *id;
            }
        }
        let parts: Vec<&str> = file.split('/').collect();
        if parts.len() > 1 {
            let owner = self.object(parts[0], parts[1].trim_end_matches(".xml"));
            if file.ends_with("/Form.xml") {
                let name = parts
                    .windows(2)
                    .find(|p| p[0] == "Forms")
                    .map(|p| p[1])
                    .unwrap_or(parts[1]);
                return self.form(owner, name, file);
            }
            if self.nodes[owner]
                .file
                .is_some_and(|i| self.files[i] == file)
            {
                return owner;
            }
            let name = parts
                .windows(2)
                .find(|p| p[0] == "Templates")
                .map(|p| p[1])
                .unwrap_or(parts.last().copied().unwrap_or(file));
            let id = self.node(
                format!("source:{file}"),
                5,
                name,
                parts[0],
                file,
                line,
                Some(owner),
                "",
            );
            let k = self.kind("contains_source", "Содержит макет / источник", "metadata");
            if !self.edge_keys.contains_key(&(owner, id, k)) {
                self.edge(owner, id, k, file, 0);
            }
            return id;
        }
        self.node(format!("source:{file}"), 5, file, "", file, line, None, "")
    }
}
fn field_path(path: &str) -> Option<String> {
    if let Some((_, rest)) = path.split_once(".TabularSection.") {
        let (tab, field) = rest.split_once(".Attribute.")?;
        return Some(format!("{tab}.{}", field.split('.').next()?));
    }
    for marker in [".Attribute.", ".Dimension.", ".Resource."] {
        if let Some((_, rest)) = path.split_once(marker) {
            return Some(rest.split('.').next()?.into());
        }
    }
    None
}
fn build(conn: &Connection) -> Result<Graph, String> {
    let mut g = Graph::default();
    for (typ, cat) in TYPES {
        g.types.insert(typ.to_lowercase(), cat.to_string());
    }
    for (name, cat) in crate::tools::реестр_объектов(conn)? {
        let id = g.object(&cat, &name);
        g.declared.push(id);
    }
    for r in table(
        conn,
        "SELECT object_name,category,file FROM object_synonyms",
    )? {
        let id = g.object(str_at(&r, 1), str_at(&r, 0));
        if g.nodes[id].file.is_none() {
            g.nodes[id].file = g.file(str_at(&r, 2));
        }
        if str_at(&r, 1) == "Subsystems" {
            let qualified = str_at(&r, 2)
                .trim_end_matches(".xml")
                .split('/')
                .filter(|p| *p != "Subsystems")
                .collect::<Vec<_>>()
                .join(".");
            g.subsystems.insert(qualified.to_lowercase(), id);
        }
    }
    for (qualified, child) in g.subsystems.clone() {
        if let Some((parent, _)) = qualified.rsplit_once('.') {
            if let Some(parent) = g.subsystems.get(parent).copied() {
                let k = g.kind("contains_subsystem", "Содержит подсистему", "metadata");
                g.edge(parent, child, k, "", 0);
            }
        }
    }
    let mut module_ids = HashMap::new();
    let mut method_ids = HashMap::new();
    let contains_module = g.kind("contains_module", "Содержит модуль", "code");
    let contains_method = g.kind("contains_method", "Содержит процедуру / функцию", "code");
    for r in table(
        conn,
        "SELECT id,rel_path,category,object_name,form_name,is_form FROM modules ORDER BY id",
    )? {
        let file = str_at(&r, 1);
        let object = g.object(str_at(&r, 2), str_at(&r, 3));
        let owner = if num_at(&r, 5) != 0 && !str_at(&r, 4).is_empty() {
            let form_file = file
                .strip_suffix("/Form/Module.bsl")
                .map(|p| format!("{p}/Form.xml"))
                .unwrap_or_default();
            g.form(object, str_at(&r, 4), &form_file)
        } else {
            object
        };
        let name = file.rsplit('/').next().unwrap_or(file);
        let id = g.node(
            format!("module:{file}"),
            1,
            name,
            str_at(&r, 2),
            file,
            0,
            Some(owner),
            "",
        );
        g.modules.insert(file.into(), id);
        module_ids.insert(num_at(&r, 0), id);
        g.edge(owner, id, contains_module, file, 0);
    }
    for r in table(
        conn,
        "SELECT id,module_id,name,line,end_line FROM methods ORDER BY module_id,line,id",
    )? {
        let Some(owner) = module_ids.get(&num_at(&r, 1)).copied() else {
            continue;
        };
        let module = g.nodes[owner].clone();
        let file = module.file.map(|i| g.files[i].clone()).unwrap_or_default();
        let name = str_at(&r, 2);
        let id = g.node(
            method_key(&file, name),
            2,
            name,
            &module.category,
            &file,
            num_at(&r, 3),
            Some(owner),
            "",
        );
        g.methods
            .insert(format!("{file}::{}", name.to_lowercase()), id);
        g.intervals
            .entry(file.clone())
            .or_default()
            .push((num_at(&r, 3), num_at(&r, 4), id));
        method_ids.insert(num_at(&r, 0), id);
        g.edge(owner, id, contains_method, &file, num_at(&r, 3));
    }
    for r in table(conn,"SELECT caller_id,callee_key,callee_name,line FROM calls WHERE callee_key IS NOT NULL ORDER BY id")?{
        let (Some(a), Some(b)) = (
            method_ids.get(&num_at(&r, 0)).copied(),
            g.methods.get(str_at(&r, 1)).copied(),
        ) else { continue; };
        let notify = str_at(&r, 2).starts_with("$notification$");
        let k = g.kind(if notify { "notification" } else { "call" },
            if notify { "Оповещение" } else { "Вызов процедуры" }, "code");
        let file = g.nodes[a].file.map(|i| g.files[i].clone()).unwrap_or_default();
        g.edge(a, b, k, &file, num_at(&r, 3));
    }
    if has(conn, "object_attributes") {
        for r in table(conn,"SELECT object_name,category,attr_name,COALESCE(ts_name,''),source_file FROM object_attributes")? {
            let owner = g.object(str_at(&r, 1), str_at(&r, 0));
            let name = if str_at(&r, 3).is_empty() { str_at(&r, 2).into() }
                else { format!("{}.{}", str_at(&r, 3), str_at(&r, 2)) };
            let id = g.field(owner, &name);
            g.nodes[id].file = g.file(str_at(&r, 4));
        }
    }
    if has(conn, "form_elements") {
        let rows=table(conn,"SELECT object_name,category,form_name,kind,element_name,element_type,handler,data_path,file,extra_json FROM form_elements ORDER BY id")?;
        let mut bindings: HashMap<(String, String), Vec<usize>> = HashMap::new();
        let mut commands = HashMap::new();
        for r in &rows {
            let owner = g.object(str_at(r, 1), str_at(r, 0));
            let file = str_at(r, 8);
            let form = g.form(owner, str_at(r, 2), file);
            let kind = str_at(r, 3);
            if kind == "attribute" {
                let id = g.member(form, 6, str_at(r, 4), file);
                let k = g.kind("form_type", "Тип реквизита формы", "forms");
                let targets: Vec<_> = str_at(r, 5)
                    .split(',')
                    .flat_map(|s| g.type_targets(s.trim()))
                    .collect();
                for target in &targets {
                    g.edge(id, *target, k, file, 0);
                }
                bindings.insert((file.into(), str_at(r, 4).to_lowercase()), targets);
            } else if kind == "command" {
                let id = g.member(form, 8, str_at(r, 4), file);
                commands.insert((file.to_string(), str_at(r, 4).to_lowercase()), id);
            } else if kind == "element" {
                g.member(form, 7, str_at(r, 4), file);
            }
        }
        for r in &rows {
            let owner = g.object(str_at(r, 1), str_at(r, 0));
            let file = str_at(r, 8);
            let form = g.form(owner, str_at(r, 2), file);
            let kind = str_at(r, 3);
            let source = if kind == "attribute" {
                g.member(form, 6, str_at(r, 4), file)
            } else if kind == "command" {
                g.member(form, 8, str_at(r, 4), file)
            } else if !str_at(r, 4).is_empty() {
                g.member(form, 7, str_at(r, 4), file)
            } else {
                form
            };
            if !str_at(r, 6).is_empty() {
                let module = file.replace("/Form.xml", "/Form/Module.bsl");
                if let Some(target) = g.method(&module, str_at(r, 6)) {
                    let k = g.kind("form_handler", "Обработчик формы / команды", "forms");
                    g.edge(source, target, k, file, 0);
                }
            }
            if let Some((prefix, field)) = str_at(r, 7).trim_start_matches('~').split_once('.') {
                if let Some(targets) = bindings.get(&(file.into(), prefix.to_lowercase())) {
                    if targets.len() == 1 {
                        let target = g.field(targets[0], field);
                        let k = g.kind("form_binding", "Привязка элемента к полю", "forms");
                        g.edge(source, target, k, file, 0);
                    }
                }
            }
            if let Ok(extra) = serde_json::from_str::<Value>(str_at(r, 9)) {
                if let Some(command) = extra["command_name"].as_str() {
                    let name = command.rsplit('.').next().unwrap_or(command).to_lowercase();
                    if let Some(target) = commands.get(&(file.into(), name)) {
                        let k = g.kind("form_command", "Команда элемента формы", "forms");
                        g.edge(source, *target, k, file, 0);
                    }
                }
            }
        }
    }
    if has(conn, "metadata_references") {
        for r in table(conn,"SELECT source_object,source_category,ref_object,ref_kind,used_in,path,line FROM metadata_references ORDER BY id")? {
            let owner = g.object(str_at(&r, 1), str_at(&r, 0));
            let source = field_path(str_at(&r, 4)).map(|f| g.field(owner, &f)).unwrap_or(owner);
            if let Some(target) = g.target(str_at(&r, 2)) {
                let label = format!("Метаданные: {}", str_at(&r, 3));
                let k = g.kind(&format!("metadata:{}", str_at(&r, 3)), &label, "metadata");
                g.edge(source, target, k, str_at(&r, 5), num_at(&r, 6));
            }
        }
    }
    for (table_name, name_column) in [
        ("defined_types", "name"),
        ("characteristic_types", "pvh_name"),
    ] {
        if has(conn, table_name) {
            let sql = format!("SELECT {name_column},type_refs_json,path FROM {table_name}");
            for r in table(conn, &sql)? {
                let owner = g.object(
                    if table_name == "defined_types" {
                        "DefinedTypes"
                    } else {
                        "ChartsOfCharacteristicTypes"
                    },
                    str_at(&r, 0),
                );
                let refs: Vec<String> =
                    serde_json::from_str(str_at(&r, 1)).map_err(|e| e.to_string())?;
                for reference in refs {
                    for target in g.type_targets(&reference) {
                        let k = g.kind("type_membership", "Состав определяемого типа", "metadata");
                        g.edge(owner, target, k, str_at(&r, 2), 0);
                    }
                }
            }
        }
    }
    if has(conn, "subsystem_content") {
        for r in table(
            conn,
            "SELECT subsystem_name,object_ref,file FROM subsystem_content",
        )? {
            let owner = g.object("Subsystems", str_at(&r, 0));
            if let Some(target) = g.target(str_at(&r, 1)) {
                let k = g.kind("subsystem", "Состав подсистемы", "metadata");
                g.edge(owner, target, k, str_at(&r, 2), 0);
            }
        }
    }
    if has(conn, "metadata_code_usages") {
        for r in table(conn,"SELECT mo.rel_path,u.object_ref,u.member_path,u.usage_kind,u.line FROM metadata_code_usages u JOIN modules mo ON mo.id=u.module_id")? {
            let source = g.source(str_at(&r, 0), num_at(&r, 4));
            if let Some(owner) = g.target(str_at(&r, 1)) {
                let target = if str_at(&r, 2).is_empty() { owner } else { g.field(owner, str_at(&r, 2)) };
                let k = g.kind(&format!("code:{}", str_at(&r, 3)),
                    &format!("Ссылка из кода: {}", str_at(&r, 3)), "code");
                g.edge(source, target, k, str_at(&r, 0), num_at(&r, 4));
            }
        }
    }
    let navigation_ready = conn
        .query_row(
            "SELECT value FROM index_meta WHERE key='navigation_state'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .as_deref()
        == Some("ready");
    if navigation_ready && has(conn, "navigation_usages") {
        for r in table(
            conn,
            "SELECT object_ref,field,kind,file,line FROM navigation_usages ORDER BY id",
        )? {
            let source = g.source(str_at(&r, 3), num_at(&r, 4));
            if let Some(owner) = g.target(str_at(&r, 0)) {
                let target = if str_at(&r, 1).is_empty() {
                    owner
                } else {
                    g.field(owner, str_at(&r, 1))
                };
                let code = str_at(&r, 2);
                let label = match code {
                    "form_path" => "Поле формы / СКД",
                    "code_field" => "Использование реквизита",
                    "bsl_query" => "Запрос BSL",
                    "xml_query" => "Запрос XML / СКД",
                    "xml_table_source" => "Основная таблица списка",
                    _ => "Ссылка на тип / значение в запросе",
                };
                let k = g.kind(
                    &format!("navigation:{code}"),
                    label,
                    if code == "form_path" { "forms" } else { "code" },
                );
                g.edge(source, target, k, str_at(&r, 3), num_at(&r, 4));
            }
        }
    } else {
        g.warnings.push(
            "Адресные связи запросов и полей отсутствуют или неактуальны: нужна пересборка индекса"
                .into(),
        );
    }
    if navigation_ready && has(conn, "navigation_rights") {
        for r in table(
            conn,
            "SELECT role,object_ref,right_name,allowed,file,line FROM navigation_rights",
        )? {
            let source = g.object("Roles", str_at(&r, 0));
            if let Some(target) = g.target(str_at(&r, 1)) {
                let flag = if num_at(&r, 3) != 0 { "true" } else { "false" };
                let k = g.kind(
                    &format!("right:{}:{flag}", str_at(&r, 2)),
                    &format!("Право {}: {flag}", str_at(&r, 2)),
                    "metadata",
                );
                g.edge(source, target, k, str_at(&r, 4), num_at(&r, 5));
            }
        }
    } else if has(conn, "role_rights") {
        for r in table(
            conn,
            "SELECT role_name,object_name,right_name,file FROM role_rights",
        )? {
            let source = g.object("Roles", str_at(&r, 0));
            if let Some(target) = g.target(str_at(&r, 1)) {
                let k = g.kind(
                    &format!("right:{}:true", str_at(&r, 2)),
                    &format!("Право {}: true", str_at(&r, 2)),
                    "metadata",
                );
                g.edge(source, target, k, str_at(&r, 3), 0);
            }
        }
    }
    for (table_name, cat) in [
        ("event_subscriptions", "EventSubscriptions"),
        ("scheduled_jobs", "ScheduledJobs"),
    ] {
        if has(conn, table_name) {
            let sql = if table_name == "event_subscriptions" {
                "SELECT name,handler_module,handler_procedure,file,source_types FROM event_subscriptions"
            } else {
                "SELECT name,handler_module,handler_procedure,file,NULL FROM scheduled_jobs"
            };
            for r in table(conn, sql)? {
                let owner = g.object(cat, str_at(&r, 0));
                g.nodes[owner].file = g.file(str_at(&r, 3));
                if let Some(target) = g.handler(str_at(&r, 1), str_at(&r, 2)) {
                    let k = g.kind("platform_handler", "Платформенный обработчик", "code");
                    g.edge(owner, target, k, str_at(&r, 3), 0);
                }
                if table_name == "event_subscriptions" {
                    let sources: Vec<String> =
                        serde_json::from_str(str_at(&r, 4)).map_err(|e| e.to_string())?;
                    for reference in sources {
                        for source in g.type_targets(&reference) {
                            let k = g.kind(
                                "subscription_source",
                                "Источник подписки (в том числе весь вид)",
                                "metadata",
                            );
                            g.edge(source, owner, k, str_at(&r, 3), 0);
                        }
                    }
                }
            }
        }
    }
    if has(conn, "exchange_plan_content") {
        for r in table(
            conn,
            "SELECT plan_name,object_ref,path FROM exchange_plan_content",
        )? {
            let owner = g.object("ExchangePlans", str_at(&r, 0));
            if let Some(target) = g.target(str_at(&r, 1)) {
                let k = g.kind("exchange_content", "Состав плана обмена", "metadata");
                g.edge(owner, target, k, str_at(&r, 2), 0);
            }
        }
    }
    if has(conn, "register_movements") {
        for r in table(
            conn,
            "SELECT document_name,register_name,file FROM register_movements",
        )? {
            let owner = g.object("Documents", str_at(&r, 0));
            let targets: Vec<_> = g
                .declared
                .iter()
                .copied()
                .filter(|i| {
                    g.nodes[*i].category.ends_with("Registers") && g.nodes[*i].name == str_at(&r, 1)
                })
                .collect();
            if targets.len() == 1 {
                let k = g.kind("register_movement", "Запись документа в регистр", "code");
                g.edge(owner, targets[0], k, str_at(&r, 2), 0);
            } else if targets.len() > 1 {
                g.warnings.push(format!(
                    "Неоднозначный вид регистра {}: связь записи не разрешена",
                    str_at(&r, 1)
                ));
            }
        }
    }
    if has(conn, "extension_overrides") {
        for r in table(conn,"SELECT source_path,target_method,ext_module_path,extension_method,ext_line FROM extension_overrides")? {
            if let (Some(target), Some(source)) = (g.method(str_at(&r, 0), str_at(&r, 1)),
                g.method(str_at(&r, 2), str_at(&r, 3))) {
                let k = g.kind("override", "Перехват расширения", "code");
                g.edge(source, target, k, str_at(&r, 2), num_at(&r, 4));
            }
        }
    }
    Ok(g)
}

fn distances(graph: &Graph, root: usize) -> Vec<usize> {
    let mut adjacent = vec![Vec::new(); graph.nodes.len()];
    for (i, e) in graph.edges.iter().enumerate() {
        adjacent[e.from].push(i);
        adjacent[e.to].push(i);
    }
    let mut distance = vec![usize::MAX; graph.nodes.len()];
    let mut queue = VecDeque::from([root]);
    distance[root] = 0;
    while let Some(id) = queue.pop_front() {
        for e in &adjacent[id] {
            let e = &graph.edges[*e];
            let next = if e.from == id { e.to } else { e.from };
            if distance[next] == usize::MAX {
                distance[next] = distance[id] + 1;
                queue.push_back(next);
            }
        }
    }
    distance
}
pub fn data(conn: &Connection, args: &Value) -> Result<Value, String> {
    let start = std::time::Instant::now();
    let graph = build(conn)?;
    let requested = if let Some(key) = args["key"].as_str() {
        key.to_string()
    } else {
        match args["kind"].as_str().unwrap_or("object") {
            "method" => method_key(
                args["module"].as_str().ok_or("нужен module")?,
                args["name"].as_str().ok_or("нужно имя")?,
            ),
            _ => {
                let key = object_key(
                    args["category"].as_str().ok_or("нужна category")?,
                    args["name"].as_str().ok_or("нужно имя")?,
                );
                if let Some(field) = args["field"].as_str() {
                    format!("field:{key}::{}", field.to_lowercase())
                } else {
                    key
                }
            }
        }
    };
    let root = *graph
        .keys
        .get(&requested)
        .ok_or("узел не найден в индексе связей")?;
    let distance = distances(&graph, root);
    let mut remap = vec![usize::MAX; graph.nodes.len()];
    let mut selected = Vec::new();
    for (i, d) in distance.iter().enumerate() {
        if *d != usize::MAX {
            remap[i] = selected.len();
            selected.push(i);
        }
    }
    let nodes: Vec<Value> = selected
        .iter()
        .map(|i| {
            let n = &graph.nodes[*i];
            json!([
                n.key,
                n.kind,
                n.name,
                n.category,
                n.file,
                n.line,
                n.owner
                    .and_then(|o| (remap[o] != usize::MAX).then_some(remap[o])),
                n.field,
                distance[*i]
            ])
        })
        .collect();
    let mut occurrences = 0;
    let edges: Vec<Value> = graph
        .edges
        .iter()
        .filter(|e| remap[e.from] != usize::MAX && remap[e.to] != usize::MAX)
        .map(|e| {
            occurrences += e.count;
            json!([remap[e.from], remap[e.to], e.kind, e.file, e.line, e.count])
        })
        .collect();
    let unresolved:i64=conn.query_row("SELECT count(*) FROM calls WHERE callee_key IS NULL AND resolution NOT IN ('platform_var','platform_global')",[],|r|r.get(0)).unwrap_or(0);
    Ok(
        json!({"root":remap[root],"nodes":nodes,"edges":edges,"files":graph.files,"kinds":graph.kinds,"node_columns":["key","kind","name","category","file","line","owner","field","distance"],"edge_columns":["from","to","kind","file","line","occurrences"],"complete_in_index":true,"truncated":false,"occurrences":occurrences,"index_nodes":graph.nodes.len(),"index_edges":graph.edges.len(),"unresolved_calls_in_index":unresolved,"warnings":graph.warnings,"elapsed_ms":start.elapsed().as_millis(),"note":"Вся достижимая компонента связей индекса в обоих направлениях. Циклы обходятся один раз. Неразрешённые вызовы не считаются доказанными связями. false в роли не означает глобальный запрет."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn адреса_полей_форм_и_вложенных_подсистем_не_смешиваются() {
        let mut g = Graph::default();
        for (typ, cat) in TYPES {
            g.types.insert(typ.to_lowercase(), cat.to_string());
        }
        let owner = g.object("Catalogs", "Товары");
        let field = g
            .target("Catalog.Товары.TabularSection.Строки.Attribute.Цена")
            .unwrap();
        assert_eq!(g.nodes[field].owner, Some(owner));
        assert_eq!(g.nodes[field].field, "Строки.Цена");
        let form = g.form(owner, "Форма", "Catalogs/Товары/Forms/Форма/Ext/Form.xml");
        assert_eq!(g.target("Catalog.Товары.Form.Форма"), Some(form));
        let child = g.object("Subsystems", "Дочерняя");
        g.subsystems.insert("родитель.дочерняя".into(), child);
        assert_eq!(
            g.target("Subsystem.Родитель.Subsystem.Дочерняя"),
            Some(child)
        );
        let config = g.target("Configuration.Тест").unwrap();
        assert_eq!(g.nodes[config].category, "Configuration");
        let source = g.source("Catalogs/Товары.xml", 10);
        assert_eq!(g.nodes[source].owner, Some(owner));
    }
    #[test]
    fn тысячи_непосредственных_соседей_не_обрезаются() {
        let mut g = Graph::default();
        let root = g.object("CommonModules", "Общий");
        let k = g.kind("r", "Связь", "code");
        for i in 0..3821 {
            let caller = g.object("CommonModules", &format!("Модуль{i}"));
            g.edge(caller, root, k, "", 1);
        }
        let found = distances(&g, root);
        assert_eq!(found.len(), 3822);
        assert_eq!(found.iter().filter(|d| **d == 1).count(), 3821);
    }
    #[test]
    fn обход_не_обрезает_циклы_и_сохраняет_параллельные_связи() {
        let mut g = Graph::default();
        let a = g.object("Catalogs", "А");
        let b = g.object("Catalogs", "Б");
        let k = g.kind("r", "Связь", "metadata");
        g.edge(a, b, k, "x", 1);
        g.edge(a, b, k, "x", 2);
        g.edge(b, a, k, "x", 3);
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].count, 2);
        let mut last = b;
        for i in 0..2501 {
            let next = g.object("Catalogs", &format!("О{i}"));
            g.edge(last, next, k, "x", 0);
            last = next;
        }
        g.edge(last, a, k, "x", 0);
        let isolated = g.object("Catalogs", "Изолированный");
        let found = distances(&g, a);
        assert_eq!(found.iter().filter(|n| **n != usize::MAX).count(), 2503);
        assert_eq!(found[isolated], usize::MAX);
    }
}
