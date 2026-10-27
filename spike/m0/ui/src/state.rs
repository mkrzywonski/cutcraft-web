//! Which answers the page may show, apart from the browser so it's tested natively (spec §13:
//! nothing from an older revision replaces something newer).

use cutcraft_web_protocol::{Geometry, PROTOCOL_VERSION, SessionSnapshot};

/// The geometry the snapshot asks for, if it has a design.
pub fn wanted_geometry(snapshot: &SessionSnapshot) -> Option<&str> {
    snapshot
        .design
        .as_ref()
        .and_then(|d| d.geometry.as_ref())
        .map(|g| g.id.as_str())
}

/// A fetched geometry is shown only if it's the one the latest snapshot asks for: responses
/// can arrive out of order, and a late one must not put an older plan back on the mat.
pub fn geometry_is_current(latest: Option<&SessionSnapshot>, got: &Geometry) -> bool {
    latest.and_then(wanted_geometry) == Some(got.id.as_str())
}

/// The identity of the artwork the snapshot shows, if any.
pub fn art_id(snapshot: &SessionSnapshot) -> Option<&str> {
    snapshot
        .design
        .as_ref()
        .and_then(|d| d.art.as_ref())
        .map(|a| a.id.as_str())
}

/// A picture is installed, and drawn, only for the artwork the latest snapshot shows: another
/// design (even at the same revision) or a late answer never paints over it.
pub fn picture_is_current(latest: Option<&SessionSnapshot>, art: &str) -> bool {
    latest.and_then(art_id) == Some(art)
}

/// Whether a snapshot may replace the one shown: not one from an older revision of the same
/// session load.
pub fn snapshot_is_newer(shown: Option<&SessionSnapshot>, new: &SessionSnapshot) -> bool {
    shown.is_none_or(|old| old.incarnation != new.incarnation || old.revision <= new.revision)
}

/// A server speaking another protocol version: the page must reload.
pub fn needs_reload(snapshot: &SessionSnapshot) -> bool {
    snapshot.protocol != PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_web_protocol::{DesignView, GeometryRef};

    fn snapshot(revision: u64, geometry: Option<&str>) -> SessionSnapshot {
        SessionSnapshot {
            protocol: PROTOCOL_VERSION,
            incarnation: "i".into(),
            revision,
            design: geometry.map(|id| DesignView {
                geometry: Some(GeometryRef { id: id.into() }),
                ..DesignView::default()
            }),
            ..SessionSnapshot::default()
        }
    }

    fn geometry(id: &str) -> Geometry {
        Geometry {
            id: id.into(),
            ..Geometry::default()
        }
    }

    /// From Grok's review of milestone 0: two changes close together start two geometry
    /// requests, and their answers can arrive in either order.
    #[test]
    fn a_late_older_geometry_never_replaces_the_current_one() {
        let latest = snapshot(3, Some("3-b"));
        assert!(!geometry_is_current(Some(&latest), &geometry("2-a")));
        assert!(geometry_is_current(Some(&latest), &geometry("3-b")));
    }

    #[test]
    fn no_geometry_is_current_without_a_design() {
        assert!(!geometry_is_current(
            Some(&snapshot(4, None)),
            &geometry("3-b")
        ));
        assert!(!geometry_is_current(None, &geometry("3-b")));
    }

    #[test]
    fn an_older_snapshot_of_the_same_load_is_ignored_but_a_new_load_wins() {
        let shown = snapshot(5, Some("5-x"));
        assert!(!snapshot_is_newer(Some(&shown), &snapshot(4, Some("4-x"))));
        assert!(snapshot_is_newer(Some(&shown), &snapshot(5, Some("5-x"))));
        let mut reloaded = snapshot(0, None);
        reloaded.incarnation = "j".into();
        assert!(snapshot_is_newer(Some(&shown), &reloaded));
    }

    /// From Codex's review of milestone 0: two different designs both start at revision 1, so
    /// a picture must be told apart by the artwork's identity, not the revision.
    #[test]
    fn a_picture_of_another_design_is_never_current() {
        let mut s = snapshot(1, Some("1-a"));
        if let Some(d) = s.design.as_mut() {
            d.art = Some(cutcraft_web_protocol::ArtView {
                id: "i-blue-1-1".into(),
                design_revision: 1,
                bounds: [0.0, 0.0, 80.0, 60.0],
                scale: 1.0,
            });
        }
        assert!(picture_is_current(Some(&s), "i-blue-1-1"));
        assert!(!picture_is_current(Some(&s), "i-red-1-1"));
        assert!(!picture_is_current(Some(&snapshot(2, None)), "i-blue-1-1"));
    }

    #[test]
    fn another_protocol_version_means_reload() {
        let mut s = snapshot(1, None);
        assert!(!needs_reload(&s));
        s.protocol = PROTOCOL_VERSION + 1;
        assert!(needs_reload(&s));
    }
}
