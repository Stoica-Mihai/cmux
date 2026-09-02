use super::*;

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Chords are listed where they are live. At the dashboard none of them do
/// anything until the prefix is down, so listing them there is noise; once
/// it is down they are one keypress away and the full list belongs there.
#[test]
fn the_chord_list_lives_in_the_prefix_row_not_the_idle_one() {
    let idle = text(&dashboard_footer(""));
    let prefix = text(&prefix_footer());

    for chord in ["=new", "=load", "=rename", "=detach", "=sidebar", "=quit"] {
        assert!(
            prefix.contains(chord),
            "the prefix row should list {chord}: {prefix}"
        );
        assert!(
            !idle.contains(chord),
            "the idle row still lists {chord}, where it does nothing: {idle}"
        );
    }

    // The idle row still has to say how to reach them.
    assert!(idle.contains(keys::PREFIX.label), "{idle}");
    assert!(idle.contains(keys::PREFIX_HELP.label), "{idle}");
    assert!(
        idle.chars().count() < prefix.chars().count(),
        "the idle row should be the shorter of the two"
    );
}

#[test]
fn a_status_message_is_appended_to_the_idle_row() {
    let plain = text(&dashboard_footer(""));
    let with_status = text(&dashboard_footer("spawned session [2]"));
    assert!(with_status.contains("spawned session [2]"));
    assert!(with_status.chars().count() > plain.chars().count());
}

/// The row said "Ctrl+A" twice: once as its own hint, once inside a status
/// message that carried a second copy of the chord list.
#[test]
fn the_prefix_is_named_once_even_with_a_status() {
    for status in ["", "spawned session [2]", "resumed session [7]"] {
        let line = text(&dashboard_footer(status));
        let named = line.matches(keys::PREFIX.label).count();
        assert_eq!(named, 1, "the prefix is named {named} times: {line}");
    }
}
