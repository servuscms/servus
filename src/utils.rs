// * Code taken from [Zola](https://www.getzola.org/) and adapted.
// * Zola's MIT license applies. See: https://github.com/getzola/zola/blob/master/LICENSE

use toml::Value as TomlValue;

#[derive(Debug)]
pub struct MergeError;

// https://github.com/getzola/zola/blob/master/components/config/src/config/mod.rs

pub fn merge(into: &mut TomlValue, from: &TomlValue) -> Result<(), MergeError> {
    match (from.is_table(), into.is_table()) {
        (false, false) => {
            // These are not tables so we have nothing to merge
            Ok(())
        }
        (true, true) => {
            // Recursively merge these tables
            let into_table = into.as_table_mut().unwrap();
            for (key, val) in from.as_table().unwrap() {
                if !into_table.contains_key(key) {
                    // An entry was missing in the first table, insert it
                    into_table.insert(key.to_string(), val.clone());
                    continue;
                }
                // Two entries to compare, recurse
                merge(into_table.get_mut(key).unwrap(), val)?;
            }
            Ok(())
        }
        _ => Err(MergeError),
    }
}
