fn find_child_obj_mut<'a>(
    obj: &'a mut keyvalues_parser::Obj<'static>,
    target_key: &str,
) -> Option<&'a mut keyvalues_parser::Obj<'static>> {
    if obj.contains_key(target_key) {
        let values = obj.get_mut(target_key)?;
        for val in values {
            if let keyvalues_parser::Value::Obj(child_obj) = val {
                return Some(child_obj);
            }
        }
        return None;
    }

    for values in obj.values_mut() {
        for val in values {
            if let keyvalues_parser::Value::Obj(child_obj) = val
                && let Some(found) = find_child_obj_mut(child_obj, target_key)
            {
                return Some(found);
            }
        }
    }

    None
}

/// Removes AppID mappings in `content` whose tool name is `nucleon` or starts with `nucleon-`.
pub fn unmap_all_nucleon_compat_mappings(content: &str) -> (String, bool) {
    let Ok(partial) = keyvalues_parser::parse(content) else {
        return (content.to_string(), false);
    };
    let mut vdf = keyvalues_parser::Vdf::from(partial).into_owned();
    let mut modified = false;

    let compat_obj = if vdf.key == "CompatToolMapping" {
        vdf.value.get_mut_obj()
    } else if let keyvalues_parser::Value::Obj(ref mut root_obj) = vdf.value {
        find_child_obj_mut(root_obj, "CompatToolMapping")
    } else {
        None
    };

    let Some(compat) = compat_obj else {
        return (content.to_string(), false);
    };

    let to_remove: Vec<String> = compat
        .iter()
        .filter(|(_appid, values)| {
            values.iter().any(|val| {
                if let keyvalues_parser::Value::Obj(entry_obj) = val {
                    entry_obj
                        .get("name")
                        .and_then(|v| v.first())
                        .and_then(|v| v.get_str())
                        .map(|s| s == "nucleon" || s.starts_with("nucleon-"))
                        .unwrap_or(false)
                } else {
                    false
                }
            })
        })
        .map(|(k, _)| k.to_string())
        .collect();

    for appid in to_remove {
        compat.remove(appid.as_str());
        modified = true;
    }

    if modified {
        (vdf.to_string(), true)
    } else {
        (content.to_string(), false)
    }
}
