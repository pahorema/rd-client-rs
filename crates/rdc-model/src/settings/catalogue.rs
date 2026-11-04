//! The ordered list of settings shown by the web UI.

use std::sync::LazyLock;

use super::field::Leaf;
use super::value::SettingKind;
use super::{
    DownloadClientSettings, GeneralSettings, ProviderSettings, TorrentDefaults, WatchSettings,
};

/// One entry of the settings catalogue: a group node or a stored setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingDescriptor {
    key: String,
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    kind: SettingKind,
    enum_options: &'static [(i32, &'static str)],
}

impl SettingDescriptor {
    /// The key: group names and the setting name joined by `:`
    /// (e.g. `General:LogLevel`, `Provider:Default:Category`); also the
    /// `Settings.SettingId` of stored settings.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Title shown by the web UI.
    pub fn display_name(&self) -> Option<&'static str> {
        self.display_name
    }

    /// Help text shown by the web UI; may contain HTML links and `\n` line
    /// breaks.
    pub fn description(&self) -> Option<&'static str> {
        self.description
    }

    /// The entry type; [`SettingKind::Object`] for group nodes.
    pub fn kind(&self) -> SettingKind {
        self.kind
    }

    /// `(value, label)` pairs in ascending order for enum settings, empty
    /// otherwise.
    pub fn enum_options(&self) -> &'static [(i32, &'static str)] {
        self.enum_options
    }

    /// Whether this entry is a stored setting rather than a group node.
    pub fn is_stored(&self) -> bool {
        self.kind != SettingKind::Object
    }
}

/// A group node of the catalogue and the settings directly below it.
struct Group {
    key: &'static str,
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    leaves: &'static [Leaf],
}

/// Every group in display order; each group node precedes its settings.
const GROUPS: &[Group] = &[
    Group {
        key: "General",
        display_name: Some("General"),
        description: Some(""),
        leaves: GeneralSettings::LEAVES,
    },
    Group {
        key: "DownloadClient",
        display_name: Some("Download Client"),
        description: Some(""),
        leaves: DownloadClientSettings::LEAVES,
    },
    Group {
        key: "Provider",
        display_name: Some("Provider"),
        description: Some(""),
        leaves: ProviderSettings::LEAVES,
    },
    Group {
        key: "Provider:Default",
        display_name: Some("Auto Import Defaults"),
        description: None,
        leaves: TorrentDefaults::LEAVES,
    },
    Group {
        key: "Integrations",
        display_name: Some("qBittorrent / *darr"),
        description: Some(
            "The following settings only apply when a torrent gets added through the qBittorrent API, \
             usually Radarr or Sonarr.",
        ),
        leaves: &[],
    },
    Group {
        key: "Integrations:Default",
        display_name: None,
        description: None,
        leaves: TorrentDefaults::LEAVES,
    },
    Group {
        key: "Gui",
        display_name: Some("GUI Defaults"),
        description: Some("Settings used when adding a torrent through the web interface."),
        leaves: &[],
    },
    Group {
        key: "Gui:Default",
        display_name: None,
        description: None,
        leaves: TorrentDefaults::LEAVES,
    },
    Group {
        key: "Watch",
        display_name: Some("Watch"),
        description: Some(
            "The following settings only apply when a torrent gets added through the watch folder.",
        ),
        leaves: WatchSettings::LEAVES,
    },
    Group {
        key: "Watch:Default",
        display_name: Some("Import Defaults"),
        description: None,
        leaves: TorrentDefaults::LEAVES,
    },
];

static CATALOGUE: LazyLock<Vec<SettingDescriptor>> = LazyLock::new(|| {
    GROUPS
        .iter()
        .flat_map(|group| {
            let node = SettingDescriptor {
                key: group.key.to_owned(),
                display_name: group.display_name,
                description: group.description,
                kind: SettingKind::Object,
                enum_options: &[],
            };
            let leaves = group.leaves.iter().map(|leaf| SettingDescriptor {
                key: format!("{}:{}", group.key, leaf.name),
                display_name: Some(leaf.display_name),
                description: Some(leaf.description),
                kind: leaf.kind,
                enum_options: leaf.options,
            });
            std::iter::once(node).chain(leaves)
        })
        .collect()
});

/// The settings catalogue in display order: every group node followed by
/// its settings, nested groups after their parent's own settings.
pub fn catalogue() -> &'static [SettingDescriptor] {
    &CATALOGUE
}
