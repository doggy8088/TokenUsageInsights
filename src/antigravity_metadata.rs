use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    pub models: HashMap<u32, String>,
    pub cwd: Option<String>,
    pub parent_session_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_role: Option<String>,
    pub initial_prompt: Option<String>,
}

pub(crate) fn is_valid_session_id(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 200 || trimmed.chars().any(char::is_control) {
        return false;
    }
    let mut components = Path::new(trimmed).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn clean_metadata_label(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() || text.len() > 200 || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_owned())
}

fn clean_metadata_prompt(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

/// Read original workspace, model, and subagent identities without using current CLI settings.
pub fn read_session_metadata(root: &Path, session_id: &str) -> rusqlite::Result<SessionMetadata> {
    if !is_valid_session_id(session_id) {
        return Ok(SessionMetadata::default());
    }
    let path = root.join("conversations").join(format!("{session_id}.db"));
    let mut metadata = match Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => {
            let mut meta = read_trajectory_metadata(&conn, session_id)?;
            meta.models = read_response_models(&conn)?;
            meta
        }
        Err(_) if matches!(path.try_exists(), Ok(false)) => {
            let mut meta = SessionMetadata::default();
            apply_subagent_descriptor_fallback(root, session_id, &mut meta, true);
            return Ok(meta);
        }
        Err(error) => return Err(error),
    };
    if metadata.parent_session_id.is_some()
        && (metadata.agent_type.is_none()
            || metadata.agent_role.is_none()
            || metadata.cwd.is_none())
    {
        apply_subagent_descriptor_fallback(root, session_id, &mut metadata, false);
    }
    Ok(metadata)
}

/// Populate missing subagent metadata from `brain/<parent_id>/.system_generated/subagents/<session_id>.json`.
/// When `scan_unknown_parent` is false, only checks the known `metadata.parent_session_id` in O(1).
pub fn apply_subagent_descriptor_fallback(
    root: &Path,
    session_id: &str,
    metadata: &mut SessionMetadata,
    scan_unknown_parent: bool,
) {
    if !is_valid_session_id(session_id) {
        return;
    }
    if let Some(parent_id) = metadata.parent_session_id.clone() {
        let descriptor_path = root
            .join("brain")
            .join(&parent_id)
            .join(".system_generated/subagents")
            .join(format!("{session_id}.json"));
        read_subagent_descriptor_file(&descriptor_path, &parent_id, session_id, metadata);
        return;
    }
    if !scan_unknown_parent {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join("brain")) else {
        return;
    };
    let mut parents: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            let parent_id = entry.file_name().to_str()?.to_owned();
            if parent_id == session_id || !is_valid_session_id(&parent_id) {
                return None;
            }
            Some((parent_id, entry.path()))
        })
        .collect();
    parents.sort_by(|a, b| a.0.cmp(&b.0));

    for (parent_id, parent_dir) in parents {
        let descriptor_path = parent_dir
            .join(".system_generated/subagents")
            .join(format!("{session_id}.json"));
        if descriptor_path.is_file()
            && read_subagent_descriptor_file(&descriptor_path, &parent_id, session_id, metadata)
        {
            break;
        }
    }
}

/// Indexes `brain/<parent_id>/.system_generated/subagents/<child_id>.json` files
/// across the Antigravity root so `sync_antigravity_usage_logs_with_cancel` can
/// include fallback descriptor revisions in its synchronization fingerprint.
pub(crate) fn index_subagent_descriptor_paths(root: &Path) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    let Ok(entries) = fs::read_dir(root.join("brain")) else {
        return index;
    };
    let mut parents: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            let parent_id = entry.file_name().to_str()?.to_owned();
            if !is_valid_session_id(&parent_id) {
                return None;
            }
            Some((parent_id, entry.path()))
        })
        .collect();
    parents.sort_by(|a, b| a.0.cmp(&b.0));

    for (parent_id, parent_dir) in parents {
        let subagents_dir = parent_dir.join(".system_generated/subagents");
        let Ok(subagents) = fs::read_dir(&subagents_dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = subagents
            .flatten()
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                file_type.is_file().then(|| entry.path())
            })
            .collect();
        children.sort();
        for descriptor_path in children {
            if descriptor_path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(child_id) = descriptor_path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if child_id == parent_id || !is_valid_session_id(child_id) {
                continue;
            }
            index.entry(child_id.to_owned()).or_insert(descriptor_path);
        }
    }
    index
}

fn read_subagent_descriptor_file(
    descriptor_path: &Path,
    parent_id: &str,
    session_id: &str,
    metadata: &mut SessionMetadata,
) -> bool {
    let Ok(raw) = fs::read_to_string(descriptor_path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    if let Some(conv_id) = json.get("conversationId").and_then(Value::as_str) {
        if conv_id.trim() != session_id {
            return false;
        }
    }
    if metadata.parent_session_id.is_none() && parent_id != session_id {
        metadata.parent_session_id = Some(parent_id.to_owned());
    }
    if let Some(descriptor) = json.get("subagentDescriptor").and_then(Value::as_object) {
        if metadata.agent_type.is_none() {
            metadata.agent_type = descriptor
                .get("typeName")
                .and_then(Value::as_str)
                .and_then(|s| clean_metadata_label(s.as_bytes()));
        }
        if metadata.agent_role.is_none() {
            metadata.agent_role = descriptor
                .get("role")
                .and_then(Value::as_str)
                .and_then(|s| clean_metadata_label(s.as_bytes()));
        }
    }
    if metadata.cwd.is_none() {
        if let Some(uris) = json.get("workspaceUris").and_then(Value::as_array) {
            let mut workspace = None;
            let mut unresolved = false;
            for uri in uris {
                let Some(path) = uri.as_str().and_then(file_uri_path) else {
                    unresolved = true;
                    continue;
                };
                if workspace.as_ref().is_some_and(|prev| prev != &path) {
                    unresolved = true;
                } else {
                    workspace = Some(path);
                }
            }
            if !unresolved {
                metadata.cwd = workspace;
            }
        }
    }
    true
}

fn merge_unique_option<T: PartialEq>(target: &mut Option<T>, unresolved: &mut bool, value: T) {
    if *unresolved {
        return;
    }
    if target.as_ref().is_some_and(|previous| previous != &value) {
        *target = None;
        *unresolved = true;
    } else {
        *target = Some(value);
    }
}

fn read_trajectory_metadata(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<SessionMetadata> {
    let mut statement = conn.prepare("PRAGMA table_info(trajectory_metadata_blob)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "data") {
        return Ok(SessionMetadata::default());
    }
    let mut statement = conn.prepare("SELECT data FROM trajectory_metadata_blob")?;
    let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
    let mut workspace = None;
    let mut workspace_unresolved = false;
    let mut parent_session_id = None;
    let mut parent_unresolved = false;
    let mut agent_type = None;
    let mut agent_type_unresolved = false;
    let mut agent_role = None;
    let mut agent_role_unresolved = false;
    let mut initial_prompt = None;
    let mut prompt_unresolved = false;

    for row in rows {
        let blob = row?;
        let Some(fields) = protobuf_fields(&blob) else {
            workspace_unresolved = true;
            parent_unresolved = true;
            agent_type_unresolved = true;
            agent_role_unresolved = true;
            prompt_unresolved = true;
            continue;
        };
        // Verified in the CLI's embedded CortexTrajectoryMetadata descriptor:
        // field 4 = agent_script (AgentScriptItem: field 1 = name);
        // field 5 = parent_conversation_id (immediate parent session UUID);
        // field 6 = root_conversation_id (top-level root session UUID);
        // field 7 = repeated workspace_uris; deprecated field 1 = workspaces,
        // whose field 1 is CortexWorkspaceMetadata.workspace_folder_absolute_uri;
        // field 8 = subagent_spec (SubagentSpec: field 1 = type_name,
        // field 2 = role, field 3 = initial_prompt).
        let uris = if fields.iter().any(|(number, _)| *number == 7) {
            fields
                .iter()
                .filter(|(number, _)| *number == 7)
                .map(|(_, value)| Some(*value))
                .collect::<Vec<_>>()
        } else {
            fields
                .iter()
                .filter(|(number, _)| *number == 1)
                .map(|(_, value)| protobuf_fields(value).and_then(|inner| bytes_field(&inner, 1)))
                .collect::<Vec<_>>()
        };
        for uri in uris {
            let Some(path) = uri
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(file_uri_path)
            else {
                workspace_unresolved = true;
                continue;
            };
            merge_unique_option(&mut workspace, &mut workspace_unresolved, path);
        }

        for (_, value) in fields.iter().filter(|(number, _)| *number == 5) {
            let Some(parent_id) = std::str::from_utf8(value)
                .ok()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                parent_unresolved = true;
                continue;
            };
            if !is_valid_session_id(parent_id) {
                parent_unresolved = true;
                continue;
            }
            if parent_id != session_id {
                merge_unique_option(
                    &mut parent_session_id,
                    &mut parent_unresolved,
                    parent_id.to_owned(),
                );
            }
        }

        let mut row_has_subagent_type = false;
        for (_, value) in fields.iter().filter(|(number, _)| *number == 8) {
            let Some(spec_fields) = protobuf_fields(value) else {
                agent_type_unresolved = true;
                agent_role_unresolved = true;
                prompt_unresolved = true;
                continue;
            };
            if let Some(raw_type) = bytes_field(&spec_fields, 1) {
                if let Some(label) = clean_metadata_label(raw_type) {
                    row_has_subagent_type = true;
                    merge_unique_option(&mut agent_type, &mut agent_type_unresolved, label);
                } else if !raw_type.is_empty() {
                    agent_type_unresolved = true;
                }
            }
            if let Some(raw_role) = bytes_field(&spec_fields, 2) {
                if let Some(label) = clean_metadata_label(raw_role) {
                    merge_unique_option(&mut agent_role, &mut agent_role_unresolved, label);
                } else if !raw_role.is_empty() {
                    agent_role_unresolved = true;
                }
            }
            if let Some(raw_prompt) = bytes_field(&spec_fields, 3) {
                if let Some(prompt) = clean_metadata_prompt(raw_prompt) {
                    merge_unique_option(&mut initial_prompt, &mut prompt_unresolved, prompt);
                }
            }
        }

        // Fall back to AgentScriptItem.name (field 4.1) when subagent_spec.type_name is absent
        // on a subagent trajectory.
        if !row_has_subagent_type
            && (fields
                .iter()
                .any(|(number, _)| *number == 5 || *number == 8))
        {
            for (_, value) in fields.iter().filter(|(number, _)| *number == 4) {
                if let Some(script_fields) = protobuf_fields(value) {
                    if let Some(raw_name) = bytes_field(&script_fields, 1) {
                        if let Some(label) = clean_metadata_label(raw_name) {
                            merge_unique_option(&mut agent_type, &mut agent_type_unresolved, label);
                        }
                    }
                }
            }
        }
    }

    Ok(SessionMetadata {
        models: HashMap::new(),
        cwd: if workspace_unresolved {
            None
        } else {
            workspace
        },
        parent_session_id: if parent_unresolved {
            None
        } else {
            parent_session_id
        },
        agent_type: if agent_type_unresolved {
            None
        } else {
            agent_type
        },
        agent_role: if agent_role_unresolved {
            None
        } else {
            agent_role
        },
        initial_prompt: if prompt_unresolved {
            None
        } else {
            initial_prompt
        },
    })
}

#[cfg(test)]
fn read_workspace(conn: &Connection) -> rusqlite::Result<Option<String>> {
    Ok(read_trajectory_metadata(conn, "test-session")?.cwd)
}

fn file_uri_path(value: &str) -> Option<String> {
    if !value.starts_with("file://") || value.chars().any(char::is_control) {
        return None;
    }
    let uri = reqwest::Url::parse(value).ok()?;
    if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
        return None;
    }
    // Decode independently of the host OS: imported sessions may come from Windows
    // while the dashboard runs on Unix (or vice versa).
    let mut decoded = Vec::new();
    let mut bytes = uri.path().as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            ((high << 4) | low) as u8
        } else {
            byte
        });
    }
    let path = String::from_utf8(decoded).ok()?;
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return None;
    }
    if let Some(host) = uri
        .host_str()
        .filter(|host| !host.is_empty() && *host != "localhost")
    {
        return Some(format!("//{host}{path}"));
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 4 && bytes[1].is_ascii_alphabetic() && bytes[2..4] == *b":/" {
        Some(path[1..].to_owned())
    } else {
        Some(path)
    }
}

fn read_response_models(conn: &Connection) -> rusqlite::Result<HashMap<u32, String>> {
    // Older/newer schemas may not expose attribution. Actual read errors (including
    // busy or corrupt databases) must propagate so sync does not cache a downgrade.
    for (table, required) in [
        ("gen_metadata", &["data"][..]),
        ("steps", &["idx", "metadata"][..]),
    ] {
        let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if required
            .iter()
            .any(|name| !columns.iter().any(|column| column == name))
        {
            return Ok(HashMap::new());
        }
    }
    // Observed in Antigravity CLI's SQLite data and embedded protobuf descriptors:
    // gen_metadata.data: field 4 = generation UUID, field 1.19 = model UID.
    // steps.metadata: field 12 = generation UUID. The steps.idx is the transcript
    // step_index; gen_metadata.idx is a different index and must not be joined to it.
    let mut generations: HashMap<Vec<u8>, Option<String>> = HashMap::new();
    let mut statement = conn.prepare("SELECT data FROM gen_metadata")?;
    let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
    for row in rows {
        let blob = row?;
        let Some(fields) = protobuf_fields(&blob) else {
            continue;
        };
        let Some(id) = bytes_field(&fields, 4).filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(inner) = bytes_field(&fields, 1).and_then(protobuf_fields) else {
            continue;
        };
        let Some(uid) = bytes_field(&inner, 19).and_then(|v| std::str::from_utf8(v).ok()) else {
            continue;
        };
        // Preserve unknown identifiers, but never expose arbitrary binary payloads.
        if uid.is_empty()
            || uid.len() > 200
            || !uid
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
        {
            continue;
        }
        let model = model_label(uid).to_owned();
        generations
            .entry(id.to_vec())
            .and_modify(|previous| {
                if previous.as_ref() != Some(&model) {
                    *previous = None;
                }
            })
            .or_insert(Some(model));
    }
    let mut result = HashMap::new();
    let mut statement =
        conn.prepare("SELECT idx, metadata FROM steps WHERE metadata IS NOT NULL")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    for row in rows {
        let (index, blob) = row?;
        let Ok(index) = u32::try_from(index) else {
            continue;
        };
        let Some(fields) = protobuf_fields(&blob) else {
            continue;
        };
        if let Some(Some(model)) = bytes_field(&fields, 12).and_then(|id| generations.get(id)) {
            result.insert(index, model.clone());
        }
    }
    Ok(result)
}

fn model_label(uid: &str) -> &str {
    // Verified with `agy models`; keep aliases explicit so unknown future models
    // cannot silently inherit an unrelated model's price.
    match uid {
        "agy-demo-medium" | "gemini-4-argon-medium" => "Gemini 4 Argon (Medium)",
        "agy-demo-high" | "gemini-4-argon-high" => "Gemini 4 Argon (High)",
        "agy-demo-low" | "gemini-4-argon-low" => "Gemini 4 Argon (Low)",
        "gemini-3.8-flash-high" => "Gemini 3.8 Flash (High)",
        "gemini-3.8-flash-medium" => "Gemini 3.8 Flash (Medium)",
        "gemini-3.8-flash-low" => "Gemini 3.8 Flash (Low)",
        "gemini-flash-demo" => "Gemini 3.7 Flash",
        "gemini-3.8-flash-demo" => "Gemini 3.8 Flash",
        _ => uid,
    }
}

fn bytes_field<'a>(fields: &[(u64, &'a [u8])], number: u64) -> Option<&'a [u8]> {
    let mut matching = fields.iter().filter(|(field, _)| *field == number);
    let value = matching.next()?.1;
    // An ambiguous identity is safer to leave unresolved.
    if matching.next().is_some() {
        None
    } else {
        Some(value)
    }
}

fn varint(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*offset)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Extract length-delimited fields, validating and skipping other wire types.
/// There is no recursive decoding of untrusted data or allocation from lengths.
fn protobuf_fields(bytes: &[u8]) -> Option<Vec<(u64, &[u8])>> {
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < bytes.len() {
        let tag = varint(bytes, &mut offset)?;
        let field = tag >> 3;
        if field == 0 || field > 0x1fff_ffff {
            return None;
        }
        let length = match tag & 7 {
            0 => {
                varint(bytes, &mut offset)?;
                continue;
            }
            1 => 8,
            2 => usize::try_from(varint(bytes, &mut offset)?).ok()?,
            5 => 4,
            _ => return None,
        };
        let end = offset.checked_add(length)?;
        let value = bytes.get(offset..end)?;
        if tag & 7 == 2 {
            result.push((field, value));
        }
        offset = end;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_varint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        while value >= 128 {
            bytes.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }

    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut bytes = encode_varint((number << 3) | 2);
        bytes.extend(encode_varint(value.len() as u64));
        bytes.extend(value);
        bytes
    }

    #[test]
    fn joins_generation_uuid_instead_of_unrelated_row_indices() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE gen_metadata(idx INTEGER, data BLOB); CREATE TABLE steps(idx INTEGER, metadata BLOB);").unwrap();
        for (index, uid) in [(1, "agy-demo-medium"), (2, "new-unknown-model")] {
            let id = format!("generation-{index}");
            let mut blob = field(4, id.as_bytes());
            blob.extend(field(1, &field(19, uid.as_bytes())));
            conn.execute(
                "INSERT INTO gen_metadata VALUES (?1, ?2)",
                rusqlite::params![index, blob],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO steps VALUES (?1, ?2)",
                rusqlite::params![index * 10, field(12, id.as_bytes())],
            )
            .unwrap();
        }
        let models = read_response_models(&conn).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[&10], "Gemini 4 Argon (Medium)");
        assert_eq!(models[&20], "new-unknown-model");
        assert!(!models.contains_key(&1));
    }

    #[test]
    fn malformed_protobuf_is_rejected_without_panicking() {
        for bytes in [
            vec![0],
            vec![0x0a, 10, 1],
            vec![0x08, 0x80],
            vec![0xff; 20],
            vec![0x0f],
            vec![0x09, 0],
        ] {
            assert!(protobuf_fields(&bytes).is_none());
        }
        let mut duplicate = field(4, b"one");
        duplicate.extend(field(4, b"two"));
        assert!(bytes_field(&protobuf_fields(&duplicate).unwrap(), 4).is_none());
    }

    #[test]
    fn rejects_session_path_traversal() {
        assert!(read_session_metadata(Path::new("."), "../invalid-session")
            .unwrap()
            .models
            .is_empty());
    }

    #[test]
    fn missing_database_does_not_get_created() {
        let root = std::env::temp_dir().join(format!(
            "antigravity-metadata-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(read_session_metadata(&root, "missing-session")
            .unwrap()
            .models
            .is_empty());
        assert!(!root.exists());
    }

    #[test]
    fn unsupported_schema_has_no_model_attribution() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE steps(idx INTEGER);")
            .unwrap();
        assert!(read_response_models(&conn).unwrap().is_empty());
    }

    #[test]
    fn unreadable_database_is_an_error_instead_of_unknown_models() {
        let root = std::env::temp_dir().join(format!(
            "antigravity-metadata-invalid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory = root.join("conversations");
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("invalid.db");
        std::fs::write(&database, b"This is deliberately not a SQLite database.").unwrap();
        let result = read_session_metadata(&root, "invalid");
        std::fs::remove_file(database).unwrap();
        std::fs::remove_dir(directory).unwrap();
        std::fs::remove_dir(root).unwrap();
        assert!(result.is_err());
    }

    fn workspace_blob(blob: &[u8]) -> Option<String> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE trajectory_metadata_blob(data BLOB);")
            .unwrap();
        conn.execute("INSERT INTO trajectory_metadata_blob VALUES (?1)", [blob])
            .unwrap();
        read_workspace(&conn).unwrap()
    }

    #[test]
    fn workspace_comes_from_session_metadata_and_prefers_current_field() {
        let mut blob = field(1, &field(1, b"file:///old/workspace"));
        blob.extend(field(7, b"file:///G:/Projects/block-out"));
        assert_eq!(
            workspace_blob(&blob).as_deref(),
            Some("G:/Projects/block-out")
        );
        assert_eq!(
            workspace_blob(&field(1, &field(1, b"file:///original/workspace"))).as_deref(),
            Some("/original/workspace")
        );
    }

    #[test]
    fn ambiguous_or_invalid_workspaces_are_not_guessed() {
        let mut duplicates = field(7, b"file:///workspace");
        duplicates.extend(field(7, b"file:///workspace"));
        assert_eq!(workspace_blob(&duplicates).as_deref(), Some("/workspace"));
        duplicates.extend(field(7, b"file:///different"));
        assert!(workspace_blob(&duplicates).is_none());
        let mut invalid = field(1, &field(1, b"file:///old/workspace"));
        invalid.extend(field(7, b"https://example.com/workspace"));
        assert!(workspace_blob(&invalid).is_none());
        assert!(workspace_blob(&[0x0a, 10, 1]).is_none());
    }

    #[test]
    fn workspace_uri_decoding_preserves_source_platform_paths() {
        for (uri, expected) in [
            ("file:///home/user/My%20Project", "/home/user/My Project"),
            ("file:///C:/Projects/%E5%B0%88%E6%A1%88", "C:/Projects/專案"),
            ("file:///home/專案", "/home/專案"),
            ("file://server/share/project", "//server/share/project"),
            ("file://localhost/home/project", "/home/project"),
        ] {
            assert_eq!(file_uri_path(uri).as_deref(), Some(expected), "{uri}");
        }
        for uri in [
            "relative/path",
            "file:relative",
            "https://example.com/path",
            "file:///bad%",
            "file:///bad%GG",
            "file:///bad%FF",
            "file:///bad%00",
            "file:///path?query",
            "file:///path#fragment",
            "file:///bad\npath",
        ] {
            assert!(file_uri_path(uri).is_none(), "{uri}");
        }
    }

    #[test]
    fn missing_optional_workspace_schema_is_supported() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(read_workspace(&conn).unwrap().is_none());
        conn.execute_batch("CREATE TABLE trajectory_metadata_blob(id TEXT);")
            .unwrap();
        assert!(read_workspace(&conn).unwrap().is_none());
    }

    #[test]
    fn reads_subagent_hierarchy_and_spec_from_trajectory_metadata_blob() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE trajectory_metadata_blob(data BLOB);")
            .unwrap();
        let mut subagent_spec = field(1, b"research");
        subagent_spec.extend(field(2, b"Spec Compliance Reviewer"));
        subagent_spec.extend(field(3, b"Review PR #73 against spec"));

        let mut blob = field(5, b"parent-session-uuid");
        blob.extend(field(6, b"root-session-uuid"));
        blob.extend(field(7, b"file:///example/project"));
        blob.extend(field(8, &subagent_spec));
        conn.execute("INSERT INTO trajectory_metadata_blob VALUES (?1)", [&blob])
            .unwrap();

        let metadata = read_trajectory_metadata(&conn, "child-session-uuid").unwrap();
        assert_eq!(metadata.cwd.as_deref(), Some("/example/project"));
        assert_eq!(
            metadata.parent_session_id.as_deref(),
            Some("parent-session-uuid")
        );
        assert_eq!(metadata.agent_type.as_deref(), Some("research"));
        assert_eq!(
            metadata.agent_role.as_deref(),
            Some("Spec Compliance Reviewer")
        );
        assert_eq!(
            metadata.initial_prompt.as_deref(),
            Some("Review PR #73 against spec")
        );

        // Self-reference guard: parent_conversation_id == session_id is ignored.
        let self_ref = read_trajectory_metadata(&conn, "parent-session-uuid").unwrap();
        assert!(self_ref.parent_session_id.is_none());
    }

    #[test]
    fn falls_back_to_parent_subagents_json_when_sqlite_db_is_absent() {
        let root = std::env::temp_dir().join(format!(
            "antigravity-subagent-fallback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let subagents_dir = root
            .join("brain")
            .join("parent-uuid")
            .join(".system_generated/subagents");
        std::fs::create_dir_all(&subagents_dir).unwrap();
        std::fs::write(
            subagents_dir.join("child-uuid.json"),
            r#"{
              "conversationId": "child-uuid",
              "subagentDescriptor": {
                "typeName": "self",
                "role": "Audio Builder Agent"
              },
              "state": "SUBAGENT_STATE_ALIVE",
              "spawnStepIndex": 12,
              "workspaceUris": ["file:///example/demo"]
            }"#,
        )
        .unwrap();

        let metadata = read_session_metadata(&root, "child-uuid").unwrap();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(metadata.parent_session_id.as_deref(), Some("parent-uuid"));
        assert_eq!(metadata.agent_type.as_deref(), Some("self"));
        assert_eq!(metadata.agent_role.as_deref(), Some("Audio Builder Agent"));
        assert_eq!(metadata.cwd.as_deref(), Some("/example/demo"));
    }
}
