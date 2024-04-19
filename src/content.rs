use serde_yaml::Value as YamlValue;
use std::{collections::HashMap, io::BufRead};

pub fn read(reader: &mut dyn BufRead) -> Option<(HashMap<String, YamlValue>, String)> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).unwrap();
        if bytes == 0 {
            return None;
        }
        if !line.trim_end_matches('\n').is_empty() {
            break;
        }
    }
    if line.trim_end_matches('\n') != "---" {
        return None;
    }
    let mut yaml_front_matter = String::new();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line.trim_end_matches('\n') == "---" {
            break;
        }
        yaml_front_matter.push_str(&line);
    }

    let front_matter: HashMap<String, YamlValue> =
        serde_yaml::from_str(&yaml_front_matter).unwrap();

    let mut content = String::new();
    let mut found_newline = false;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        let is_empty_line = line.trim_end_matches('\n').is_empty();
        if found_newline && is_empty_line {
            break;
        }
        if is_empty_line {
            found_newline = true;
        }
        content.push_str(&line);
    }

    Some((front_matter, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::BufReader;

    #[test]
    fn test_read() {
        let content = "---\n---\nqwerty";
        let (front_matter, content) = read(&mut BufReader::new(content.as_bytes())).unwrap();

        assert_eq!(front_matter.len(), 0);
        assert_eq!(content, "qwerty");

        let content = "---\nasdf: ghjk\n---\nqwerty";
        let (front_matter, content) = read(&mut BufReader::new(content.as_bytes())).unwrap();

        assert_eq!(front_matter.len(), 1);
        assert_eq!(front_matter.get("asdf").unwrap().as_str().unwrap(), "ghjk");
        assert_eq!(content, "qwerty");
    }
}
