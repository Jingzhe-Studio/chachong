use std::{env, path::PathBuf};

pub fn libreoffice_available() -> bool {
    executable_candidates()
        .into_iter()
        .any(|path| path.is_file())
}

fn executable_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(paths) = env::var_os("PATH") {
        for directory in env::split_paths(&paths) {
            candidates.push(directory.join("soffice.exe"));
            candidates.push(directory.join("soffice"));
            candidates.push(directory.join("libreoffice.exe"));
            candidates.push(directory.join("libreoffice"));
        }
    }

    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env::var_os(variable) {
            candidates.push(
                PathBuf::from(root)
                    .join("LibreOffice")
                    .join("program")
                    .join("soffice.exe"),
            );
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_generation_never_panics() {
        let _ = executable_candidates();
    }
}
