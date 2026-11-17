use std::collections::BTreeMap;

use crate::names::{are_duplicate_names, choose_keep};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub keep: String,
    pub remove: Vec<String>,
}

/// Group paths that share a parent folder, a normalized name, and an MD5.
/// Paths use `/`. A same name with a different MD5 is not proposed.
pub fn duplicate_groups(files: &[(String, String)]) -> Vec<DuplicateGroup> {
    let mut by_folder: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (path, md5) in files {
        let folder = parent_folder(path);
        by_folder.entry(folder).or_default().push((path.clone(), md5.clone()));
    }
    let mut groups = Vec::new();
    for folder_files in by_folder.values() {
        let mut used = vec![false; folder_files.len()];
        for i in 0..folder_files.len() {
            if used[i] {
                continue;
            }
            let mut members = vec![i];
            for j in (i + 1)..folder_files.len() {
                if used[j] {
                    continue;
                }
                let (left, left_md5) = &folder_files[i];
                let (right, right_md5) = &folder_files[j];
                if left_md5 == right_md5 && are_duplicate_names(file_name(left), file_name(right)) {
                    members.push(j);
                }
            }
            if members.len() < 2 {
                continue;
            }
            for index in &members {
                used[*index] = true;
            }
            let names: Vec<String> = members.iter().map(|index| file_name(&folder_files[*index].0)).map(str::to_string).collect();
            let keep_name = choose_keep(&names);
            let keep_path = members
                .iter()
                .map(|index| &folder_files[*index].0)
                .find(|path| file_name(path) == keep_name)
                .cloned()
                .unwrap_or_else(|| folder_files[members[0]].0.clone());
            let mut remove = members
                .iter()
                .map(|index| folder_files[*index].0.clone())
                .filter(|path| path != &keep_path)
                .collect::<Vec<_>>();
            remove.sort();
            groups.push(DuplicateGroup { keep: keep_path, remove });
        }
    }
    groups
}

/// Recorded destination paths that are absent from the new snapshot.
/// The first run has an empty recorded list, so nothing is proposed.
pub fn removal_candidates(recorded_before: &[String], snapshot: &[String]) -> Vec<String> {
    let mut missing = recorded_before
        .iter()
        .filter(|path| !snapshot.iter().any(|snap| snap == *path))
        .cloned()
        .collect::<Vec<_>>();
    missing.sort();
    missing.dedup();
    missing
}

fn parent_folder(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..index].to_string(),
        None => String::new(),
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_md5_proposes_the_plain_name() {
        let files = vec![
            ("2024-06/IMG_0381(1).jpg".to_string(), "aaa".to_string()),
            ("2024-06/IMG_0381.jpg".to_string(), "aaa".to_string()),
        ];
        let groups = duplicate_groups(&files);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].keep, "2024-06/IMG_0381.jpg");
        assert_eq!(groups[0].remove, vec!["2024-06/IMG_0381(1).jpg"]);
    }

    #[test]
    fn different_md5_proposes_nothing() {
        let files = vec![
            ("IMG_0381(1).jpg".to_string(), "aaa".to_string()),
            ("IMG_0381.jpg".to_string(), "bbb".to_string()),
        ];
        assert!(duplicate_groups(&files).is_empty());
    }

    #[test]
    fn same_bytes_in_different_folders_are_kept() {
        let files = vec![
            ("2024-06/IMG_0381.jpg".to_string(), "aaa".to_string()),
            ("2024-07/IMG_0381.jpg".to_string(), "aaa".to_string()),
        ];
        assert!(duplicate_groups(&files).is_empty());
    }

    #[test]
    fn removal_on_a_first_run_is_empty() {
        assert!(removal_candidates(&[], &["a.jpg".into()]).is_empty());
    }

    #[test]
    fn removal_proposes_only_recorded_files_missing_from_the_snapshot() {
        let recorded = vec!["gone.jpg".into(), "kept.jpg".into()];
        let snapshot = vec!["kept.jpg".into(), "brand-new.jpg".into()];
        assert_eq!(removal_candidates(&recorded, &snapshot), vec!["gone.jpg".to_string()]);
    }
}
