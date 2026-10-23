//! The line each step that isn't a tool of its own gets in a DAW's plan tool description.

use kumi_common::js::string::{head, trim};
use kumi_types::contracts::JsonObject;
use serde_json::{json, Value};

/// The most of a step's line, and of what it does: its fields come before more words about it.
const LINE: usize = 170;
const WHAT: usize = 72;

/// One line a step that isn't a tool of its own gets in make_changes' description: its name, what it does, and
/// the fields its input takes (`?` after one it can leave out), from its own schema.
pub fn step_line(name: &str, description: &str, schema: &JsonObject) -> String {
    line_of(name, clause(description), schema)
}

/// The same line for a step of Kumi's own bridges (REAPER, Renoise): what it does is its first clause whole, the asides
/// in parentheses left out rather than ending it ("Add (insert), delete or move slice markers" says all three).
pub fn own_step_line(name: &str, description: &str, schema: &JsonObject) -> String {
    line_of(name, whole_clause(description), schema)
}

fn line_of(name: &str, mut what: String, schema: &JsonObject) -> String {
    if what.len() > WHAT {
        let cut = head(&what, WHAT);
        what = format!("{}…", cut.rfind(' ').filter(|at| *at > WHAT / 2).map_or(cut.as_str(), |at| &cut[..at]));
    }
    let fields = crate::kernel::schema::fields(schema);
    let line = if fields.is_empty() { format!("{name}: {what}") } else { format!("{name}: {what} ({})", fields.join(", ")) };
    if line.len() > LINE {
        let cut = head(&line, LINE - 1);
        format!("{}…)", cut.trim_end_matches([',', ' ', '?']).trim_end_matches(|c: char| c.is_alphanumeric() || c == '_'))
    } else {
        line
    }
}

/// The plan tool's description and schema: `base`'s, with every step it can run (and wait) in its steps' tool enum,
/// and a line each (`step_line`) for those that aren't tools of their own.
pub fn plan_tool(base: &str, schema: &Value, mut tools: Vec<String>, lines: &[String]) -> (String, JsonObject) {
    let mut schema = schema.clone();
    tools.push("wait".into());
    schema["properties"]["steps"]["items"]["properties"]["tool"]["enum"] = json!(tools);
    let description = if lines.is_empty() {
        base.to_string()
    } else {
        format!(
            "{base} Steps that aren't tools of your own take these inputs (`?`: may be left out; find_tools gives a step's whole input): {}.",
            lines.join("; ")
        )
    };
    (description, schema.as_object().unwrap().clone())
}

/// What a tool does, from its description: up to the end of its first sentence or clause.
fn clause(description: &str) -> String {
    let text = trim(description);
    let end = [". ", ": ", "; ", " (", " — "].iter().filter_map(|stop| text.find(stop)).min().unwrap_or(text.len());
    trim(text[..end].trim_end_matches('.')).to_string()
}

/// What a tool does, from its description: its first sentence or clause, ended only outside parentheses, with what's in
/// them left out. Parentheses that don't close end it as `clause` does.
fn whole_clause(description: &str) -> String {
    let text = trim(description);
    let (mut depth, mut end, mut what) = (0usize, text.len(), String::new());
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 && [". ", ": ", "; ", " — "].iter().any(|stop| text[at..].starts_with(stop)) => {
                end = at;
                break;
            }
            _ => {}
        }
    }
    if depth > 0 {
        return clause(description);
    }
    depth = 0;
    for c in text[..end].chars() {
        match c {
            '(' => {
                if depth == 0 && what.ends_with(' ') {
                    what.pop();
                }
                depth += 1;
            }
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => what.push(c),
            _ => {}
        }
    }
    trim(what.trim_end_matches('.')).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_steps_line_names_its_fields_required_first() {
        let schema = json!({"type":"object","required":["trackRef"],"properties":{
            "enabled":{"type":"boolean"},"trackRef":{"type":"string"},"root":{"type":"integer"}
        }});
        assert_eq!(
            step_line("set_scale", "Set Scale Mode and its root. More words.", schema.as_object().unwrap()),
            "set_scale: Set Scale Mode and its root (trackRef, enabled?, root?)"
        );
        assert_eq!(step_line("wait", "Wait.", &JsonObject::new()), "wait: Wait");
        let long = step_line("edit", &"word ".repeat(80), schema.as_object().unwrap());
        assert!(long.starts_with("edit: word word") && long.ends_with("… (trackRef, enabled?, root?)"), "{long}");
        let many: serde_json::Map<String, serde_json::Value> = (0..40).map(|n| (format!("field{n}"), json!({"type": "number"}))).collect();
        let wide = step_line("wide", "Many fields.", &json!({"type":"object","properties":many}).as_object().unwrap().clone());
        assert!(wide.len() <= LINE + 4 && wide.ends_with("…)") && !wide.contains("?…"), "{wide}");
    }

    #[test]
    fn an_own_bridges_step_line_keeps_its_first_clause_whole_without_its_asides() {
        let schema = json!({"type":"object","required":["ref"],"properties":{"ref":{"type":"string"}}});
        let schema = schema.as_object().unwrap();
        let slices = "Add (insert), delete or move ({from, to}) slice markers of a sample (from 1): Renoise makes a sample of each.";
        assert_eq!(own_step_line("sample_slices", slices, schema), "sample_slices: Add, delete or move slice markers of a sample (ref)");
        assert_eq!(step_line("sample_slices", slices, schema), "sample_slices: Add (ref)", "Live's lines stay as they were");
        assert_eq!(
            own_step_line("track_channels", "Give a track (ref) channels (2 to 128). More.", schema),
            "track_channels: Give a track channels (ref)"
        );
        // Parentheses that never close: the clause ends at the first, as Live's does.
        assert_eq!(own_step_line("odd", "Make a thing (or two; more", schema), "odd: Make a thing (ref)");
    }
}
