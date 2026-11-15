//! Tinta: colour categories, as mail programs offer them. A category is a name and a colour kept
//! with the mailbox (`Store::categories`); a message names the categories it has. They stay on
//! this computer; search finds them with `category:`.

use ibercorreo_core::mail::{CATEGORY_COLORS, MAX_CATEGORIES, MAX_MESSAGE_CATEGORIES, category_name};
use ibercorreo_core::{Category, Error, Result};
use serde_json::{Value, json};

use super::{bool_param, required_str, str_param, target_message};
use crate::Session;

/// The colour a request names: a name from [`CATEGORY_COLORS`] or its index.
fn color_param(p: &Value) -> Result<Option<u8>> {
    let bad = || Error::invalid("color", format!("expected one of {}", CATEGORY_COLORS.join(", ")));
    match p.get("color") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => {
            let name = name.trim().to_ascii_lowercase();
            let name = if name == "gray" { "grey".to_string() } else { name };
            CATEGORY_COLORS.iter().position(|c| *c == name).and_then(|i| u8::try_from(i).ok()).map(Some).ok_or_else(bad)
        }
        Some(Value::Number(n)) => n
            .as_u64()
            .filter(|i| usize::try_from(*i).is_ok_and(|i| i < CATEGORY_COLORS.len()))
            .and_then(|i| u8::try_from(i).ok())
            .map(Some)
            .ok_or_else(bad),
        Some(_) => Err(bad()),
    }
}

fn name_param(p: &Value, key: &'static str) -> Result<String> {
    category_name(required_str(p, key)?).ok_or_else(|| Error::invalid(key, "a category needs a name"))
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn category_json(s: &Session, c: &Category) -> Value {
    let count = s.store.all_messages().filter(|m| m.has_category(&c.name)).count();
    json!({ "name": c.name, "color": CATEGORY_COLORS.get(usize::from(c.color)).copied().unwrap_or("grey"), "messages": count })
}

/// The first colour no category has yet, or the next one round.
fn next_color(s: &Session) -> u8 {
    let free = (0..CATEGORY_COLORS.len()).find(|i| !s.store.categories.iter().any(|c| usize::from(c.color) == *i));
    let index = free.unwrap_or(s.store.categories.len() % CATEGORY_COLORS.len());
    u8::try_from(index).unwrap_or(0)
}

/// Add a category; its name must be new.
fn add(s: &mut Session, name: String, color: Option<u8>) -> Result<Category> {
    if s.store.categories.iter().any(|c| same(&c.name, &name)) {
        return Err(Error::invalid("name", format!("there is already a category called {name}")));
    }
    if s.store.categories.len() >= MAX_CATEGORIES {
        return Err(Error::invalid("name", format!("at most {MAX_CATEGORIES} categories can be kept")));
    }
    let category = Category { name, color: color.unwrap_or_else(|| next_color(s)) };
    s.store.categories.push(category.clone());
    Ok(category)
}

pub(super) fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!(s.store.categories.iter().map(|c| category_json(s, c)).collect::<Vec<_>>()))
}

pub(super) fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let category = add(s, name, color_param(p)?)?;
    s.status = "Category created".into();
    Ok(category_json(s, &category))
}

pub(super) fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let to = str_param(p, "to")?.map(|t| category_name(t).ok_or_else(|| Error::invalid("to", "a category needs a name"))).transpose()?;
    let color = color_param(p)?;
    let index = s
        .store
        .categories
        .iter()
        .position(|c| same(&c.name, &name))
        .ok_or_else(|| Error::invalid("name", format!("no category is called {name}")))?;
    if let Some(to) = &to
        && s.store.categories.iter().enumerate().any(|(i, c)| i != index && same(&c.name, to))
    {
        return Err(Error::invalid("to", format!("there is already a category called {to}")));
    }
    let Some(category) = s.store.categories.get_mut(index) else { return Err(Error::invalid("name", "no such category")) };
    let old = category.name.clone();
    if let Some(c) = color {
        category.color = c;
    }
    if let Some(to) = to {
        category.name.clone_from(&to);
        let ids: Vec<_> = s.store.all_messages().filter(|m| m.has_category(&old)).map(|m| m.id).collect();
        for id in ids {
            let m = s.store.message_mut(id)?;
            for c in &mut m.categories {
                if same(c, &old) {
                    c.clone_from(&to);
                }
            }
        }
    }
    let category = s.store.categories.get(index).cloned().ok_or_else(|| Error::invalid("name", "no such category"))?;
    s.status = "Category changed".into();
    Ok(category_json(s, &category))
}

pub(super) fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let before = s.store.categories.len();
    s.store.categories.retain(|c| !same(&c.name, &name));
    if s.store.categories.len() == before {
        return Err(Error::invalid("name", format!("no category is called {name}")));
    }
    let ids: Vec<_> = s.store.all_messages().filter(|m| m.has_category(&name)).map(|m| m.id).collect();
    for id in &ids {
        s.store.message_mut(*id)?.categories.retain(|c| !same(c, &name));
    }
    s.status = "Category deleted".into();
    Ok(json!({ "name": name, "messages": ids.len() }))
}

/// Give a message a category or take it away; a new name makes a new category.
pub(super) fn categorize(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_message(s, p)?;
    let name = name_param(p, "category")?;
    let has = s.store.message(id)?.has_category(&name);
    let on = bool_param(p, "on")?.unwrap_or(!has);
    // The category's own spelling, made when it is new.
    let name = match s.store.categories.iter().find(|c| same(&c.name, &name)) {
        Some(c) => c.name.clone(),
        None if on => add(s, name, None)?.name,
        None => name,
    };
    let m = s.store.message_mut(id)?;
    if on && !has {
        if m.categories.len() >= MAX_MESSAGE_CATEGORIES {
            return Err(Error::invalid("category", format!("a message can have at most {MAX_MESSAGE_CATEGORIES} categories")));
        }
        m.categories.push(name.clone());
    } else if !on {
        m.categories.retain(|c| !same(c, &name));
    }
    let categories = m.categories.clone();
    Ok(json!({ "id": id.0, "category": name, "on": on, "categories": categories }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::find;
    use ibercorreo_core::Date;

    fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
        (find(id).unwrap().run)(s, &p)
    }

    #[test]
    fn categories_are_made_given_renamed_and_deleted() {
        let today = Date::new(2026, 11, 15).unwrap();
        let mut s = Session::new(crate::sample::store(today).unwrap(), today);
        let id = s.store.all_messages().next().map(|m| m.id).unwrap();

        let made = run(&mut s, "category.create", json!({ "name": "  Facturas \n", "color": "green" })).unwrap();
        assert_eq!(made["name"], "Facturas");
        assert_eq!(made["color"], "green");
        assert!(run(&mut s, "category.create", json!({ "name": "facturas" })).is_err(), "names are unique, whatever the case");
        assert!(run(&mut s, "category.create", json!({ "name": "x", "color": "beige" })).is_err());
        assert!(run(&mut s, "category.create", json!({ "name": " \t " })).is_err());

        // Giving a category toggles it; a new name makes a new category with a colour of its own.
        let on = run(&mut s, "message.categorize", json!({ "id": id.0, "category": "FACTURAS" })).unwrap();
        assert_eq!(on["categories"], json!(["Facturas"]), "the category's own spelling");
        run(&mut s, "message.categorize", json!({ "id": id.0, "category": "Viaje" })).unwrap();
        assert_eq!(s.store.categories.len(), 2);
        assert_ne!(s.store.categories.first().map(|c| c.color), s.store.categories.get(1).map(|c| c.color));
        let ids = |s: &Session, q: &str| s.store.search(q).into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&s, "category:viaje"), [id], "search finds it");

        run(&mut s, "category.edit", json!({ "name": "viaje", "to": "Viajes", "color": 5 })).unwrap();
        assert!(s.store.message(id).unwrap().has_category("Viajes"));
        assert!(run(&mut s, "category.edit", json!({ "name": "Viajes", "to": "facturas" })).is_err(), "no two of one name");

        let off = run(&mut s, "message.categorize", json!({ "id": id.0, "category": "Facturas" })).unwrap();
        assert_eq!(off["on"], false);
        let gone = run(&mut s, "category.delete", json!({ "name": "viajes" })).unwrap();
        assert_eq!(gone["messages"], 1);
        assert!(s.store.message(id).unwrap().categories.is_empty(), "deleting takes it off every message");
        assert_eq!(run(&mut s, "category.list", json!({})).unwrap().as_array().map(Vec::len), Some(1));
    }
}
