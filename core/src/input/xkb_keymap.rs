//! A keymap with one key per character to type (roadmap F4), for a Wayland
//! virtual keyboard: the way wtype types any character, whatever the
//! desktop's layout. Pure, so it is tested on every OS.

/// Distinct characters one keymap holds: keycodes 9 up to 255, with room.
pub const MAX_KEYS: usize = 200;

/// The keysym name xkbcommon reads for `ch`, or `None` for a control
/// character that types nothing. Unicode keysyms (`U263A`) name every
/// character; xkbcommon maps Latin-1 ones to their legacy keysyms itself.
pub fn keysym_name(ch: char) -> Option<String> {
    match ch {
        // A CR, alone or before an LF, types nothing: one Return per line.
        '\n' => Some("Return".to_string()),
        '\t' => Some("Tab".to_string()),
        c if c.is_control() => None,
        c => Some(format!("U{:04X}", c as u32)),
    }
}

/// The keymap for `keys`, each the next keycode from 9 (evdev code
/// `1 + index`), or `None` when there are none or too many.
pub fn keymap_for(keys: &[char]) -> Option<String> {
    if keys.is_empty() || keys.len() > MAX_KEYS {
        return None;
    }
    let names: Vec<String> = keys
        .iter()
        .map(|&c| keysym_name(c))
        .collect::<Option<_>>()?;
    let mut map = String::from("xkb_keymap {\nxkb_keycodes \"(unnamed)\" {\nminimum = 8;\n");
    map.push_str(&format!("maximum = {};\n", 9 + keys.len()));
    for i in 0..keys.len() {
        map.push_str(&format!("<K{}> = {};\n", i + 1, i + 9));
    }
    map.push_str("};\nxkb_types \"(unnamed)\" { include \"complete\" };\n");
    map.push_str("xkb_compatibility \"(unnamed)\" { include \"complete\" };\n");
    map.push_str("xkb_symbols \"(unnamed)\" {\n");
    for (i, name) in names.iter().enumerate() {
        map.push_str(&format!("key <K{}> {{[ {} ]}};\n", i + 1, name));
    }
    map.push_str("};\n};\n");
    Some(map)
}

/// `text` as runs that each fit one keymap: the distinct characters of the
/// run, in order of first use, and each character's index among them.
/// Characters that type nothing are left out.
pub fn runs(text: &str) -> Vec<(Vec<char>, Vec<usize>)> {
    let mut runs = Vec::new();
    let mut keys: Vec<char> = Vec::new();
    let mut presses: Vec<usize> = Vec::new();
    for ch in text.chars().filter(|&c| keysym_name(c).is_some()) {
        let index = match keys.iter().position(|&k| k == ch) {
            Some(i) => i,
            None => {
                if keys.len() == MAX_KEYS {
                    runs.push((std::mem::take(&mut keys), std::mem::take(&mut presses)));
                }
                keys.push(ch);
                keys.len() - 1
            }
        };
        presses.push(index);
    }
    if !presses.is_empty() {
        runs.push((keys, presses));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_is_a_key_of_its_own() {
        assert_eq!(keysym_name('A').as_deref(), Some("U0041"));
        assert_eq!(keysym_name('é').as_deref(), Some("U00E9"));
        assert_eq!(keysym_name('த').as_deref(), Some("U0BA4"));
        assert_eq!(keysym_name('😀').as_deref(), Some("U1F600"));
        assert_eq!(keysym_name('\n').as_deref(), Some("Return"));
        assert_eq!(keysym_name('\r'), None, "CRLF is one Return");
        assert_eq!(keysym_name('\u{7}'), None);
        let map = keymap_for(&['h', 'é']).unwrap();
        assert!(map.contains("maximum = 11;"));
        assert!(map.contains("<K2> = 10;"));
        assert!(map.contains("key <K2> {[ U00E9 ]};"));
        assert!(keymap_for(&[]).is_none());
    }

    #[test]
    fn text_is_cut_into_runs_that_fit_a_keymap() {
        let r = runs("abca\u{7}");
        assert_eq!(r, vec![(vec!['a', 'b', 'c'], vec![0, 1, 2, 0])]);
        let long: String = (0..(MAX_KEYS as u32 + 5))
            .map(|i| char::from_u32(0x4E00 + i).unwrap())
            .collect();
        let r = runs(&long);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0.len(), MAX_KEYS);
        assert_eq!(r[1].1, vec![0, 1, 2, 3, 4]);
        assert!(runs("").is_empty());
    }
}
