//! Дерево из ChildObjects исходной выгрузки: порядок объектов не сортируется.
use quick_xml::{events::Event, Reader};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Default, Serialize)]
struct Entry {
    kind: String,
    name: String,
    children: Vec<Entry>,
}

fn children(xml: &str) -> Result<Vec<Entry>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut tags: Vec<String> = Vec::new();
    let mut entries: Vec<(usize, Entry)> = Vec::new();
    let mut result = Vec::new();
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if tags.last().is_some_and(|t| t == "ChildObjects") {
                    entries.push((
                        tags.len() + 1,
                        Entry {
                            kind: name.clone(),
                            ..Default::default()
                        },
                    ));
                }
                tags.push(name);
            }
            Event::Text(t) => {
                if let Some((depth, entry)) = entries.last_mut() {
                    if tags.len() == *depth
                        || (tags.len() == *depth + 2 && tags.last().is_some_and(|t| t == "Name"))
                    {
                        entry.name = t.unescape().map_err(|e| e.to_string())?.into_owned();
                    }
                }
            }
            Event::End(_) => {
                if entries
                    .last()
                    .is_some_and(|(depth, _)| *depth == tags.len())
                {
                    let (_, entry) = entries.pop().unwrap();
                    if let Some((_, parent)) = entries.last_mut() {
                        parent.children.push(entry);
                    } else {
                        result.push(entry);
                    }
                }
                tags.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(result)
}

fn folder(kind: &str) -> String {
    match kind {
        "ChartOfCharacteristicTypes" => "ChartsOfCharacteristicTypes".into(),
        "ChartOfAccounts" => "ChartsOfAccounts".into(),
        "ChartOfCalculationTypes" => "ChartsOfCalculationTypes".into(),
        "FilterCriterion" => "FilterCriteria".into(),
        "WSReference" => "WSReferences".into(),
        "BusinessProcess" => "BusinessProcesses".into(),
        k => format!("{k}s"),
    }
}

pub fn data(
    conn: &Connection,
    kind: Option<&str>,
    name: Option<&str>,
    module: Option<&str>,
) -> Result<Value, String> {
    let source: String = conn
        .query_row(
            "SELECT value FROM index_meta WHERE key='source_path'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let root = std::path::Path::new(&source);
    let xml = std::fs::read_to_string(root.join("Configuration.xml"))
        .map_err(|e| format!("Configuration.xml недоступен: {e}"))?;
    let entries = children(&xml)?;
    if let Some(module) = module {
        let mut st = conn.prepare("SELECT m.name, m.type, m.line FROM methods m JOIN modules mo ON mo.id=m.module_id WHERE mo.rel_path=?1 ORDER BY m.line, m.id").map_err(|e| e.to_string())?;
        let methods = st.query_map([module], |r| Ok(json!({"name":r.get::<_,String>(0)?,"type":r.get::<_,String>(1)?,"line":r.get::<_,i64>(2)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        return Ok(json!({"methods":methods}));
    }
    let Some(kind) = kind else {
        let mut kinds: Vec<Value> = Vec::new();
        for entry in &entries {
            if let Some(row) = kinds.iter_mut().find(|r| r["kind"] == entry.kind) {
                row["count"] = json!(row["count"].as_u64().unwrap_or(0) + 1);
            } else {
                kinds.push(json!({"kind":entry.kind,"category":folder(&entry.kind),"count":1}));
            }
        }
        return Ok(
            json!({"categories":kinds,"count":entries.len(),"order":"Configuration.xml/ChildObjects"}),
        );
    };
    let selected: Vec<_> = entries.iter().filter(|e| e.kind == kind).collect();
    let Some(name) = name else {
        return Ok(json!({"objects":selected,"category":folder(kind)}));
    };
    if !selected.iter().any(|e| e.name == name) {
        return Err("объект не найден в Configuration.xml".into());
    }
    let category = folder(kind);
    // Путь строится только из проверенной записи ChildObjects.
    if name.contains(['/', '\\']) || name == ".." {
        return Err("некорректное имя объекта".into());
    }
    let xml = std::fs::read_to_string(root.join(&category).join(format!("{name}.xml")))
        .map_err(|e| e.to_string())?;
    let mut st = conn.prepare("SELECT rel_path, module_type, form_name FROM modules WHERE category=?1 AND object_name=?2 ORDER BY is_form, rel_path").map_err(|e|e.to_string())?;
    let modules = st.query_map([&category,name], |r| Ok(json!({"path":r.get::<_,String>(0)?,"type":r.get::<_,Option<String>>(1)?,"form":r.get::<_,Option<String>>(2)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
    Ok(json!({"children":children(&xml)?,"modules":modules,"category":category,"name":name}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn порядок_и_вложенность_из_xml() {
        let entries = children("<MetaDataObject><Catalog><ChildObjects><Attribute><Properties><Name>Я</Name></Properties></Attribute><Attribute><Properties><Name>А</Name></Properties></Attribute><TabularSection><Properties><Name>Строки</Name></Properties><ChildObjects><Attribute><Properties><Name>Количество</Name></Properties></Attribute></ChildObjects></TabularSection><Form>Форма</Form></ChildObjects></Catalog></MetaDataObject>").unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Я", "А", "Строки", "Форма"]
        );
        assert_eq!(entries[2].children[0].name, "Количество");
        assert_eq!(
            folder("ChartOfCharacteristicTypes"),
            "ChartsOfCharacteristicTypes"
        );
    }
}
