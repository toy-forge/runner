//! Parses every theme file in a directory and reports problems (validation aid).
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_default();
    let (mut ok, mut bad, mut sels) = (0, 0, 0);
    let mut paths: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("tmTheme")) { continue; }
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| scratch_theme::Theme::from_plist(&b).map_err(|e| e.to_string())) {
            Ok(t) => {
                ok += 1;
                sels += t.items.len();
                let r = t.compile();
                if !t.warnings.is_empty() || !r.warnings().is_empty() { println!("{}: {:?} {:?}", t.name, t.warnings, r.warnings()); }
                // Round trip.
                let again = scratch_theme::Theme::from_plist(t.to_plist_xml().unwrap_or_default().as_bytes());
                if again.map(|a| a.items.len()) .ok() != Some(t.items.len()) { println!("{}: round trip differs", t.name); }
            }
            Err(e) => { bad += 1; println!("{}: {e}", p.display()); }
        }
    }
    println!("parsed {ok} themes ({sels} items), {bad} failed");
}
