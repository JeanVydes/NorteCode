//! Virtual geometry for the settings page.

/// Sections in their display order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsSection {
    Preferences,
    Host,
    Agents,
    Plugins,
}

impl SettingsSection {
    pub(crate) const ALL: [Self; 4] = [Self::Preferences, Self::Host, Self::Agents, Self::Plugins];

    fn index(self) -> usize {
        match self {
            Self::Preferences => 0,
            Self::Host => 1,
            Self::Agents => 2,
            Self::Plugins => 3,
        }
    }
}

/// The vertical span of one section in the virtual page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SectionRect {
    pub(crate) y: u16,
    pub(crate) height: u16,
}

/// Computed once per render and reused for click hit-testing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SettingsLayout {
    sections: [SectionRect; 4],
    pref_rows_y: u16,
    plugin_rows_y: u16,
    total_height: u16,
    label_width: u16,
    value_x: u16,
}

impl SettingsLayout {
    pub(crate) const MARGIN: u16 = 2;
    const GAP: u16 = 1;
    const TABLE_HEADER: u16 = 2;
    const PANEL_BORDER: u16 = 2;

    pub(crate) fn new(
        width: u16,
        hosts: usize,
        plugins: usize,
        agents: usize,
        preferences: usize,
    ) -> Self {
        let heights = [
            1_u16.saturating_add(rows(preferences)),
            1_u16
                .saturating_add(Self::PANEL_BORDER)
                .saturating_add(Self::TABLE_HEADER)
                .saturating_add(rows(hosts)),
            1_u16
                .saturating_add(Self::PANEL_BORDER)
                .saturating_add(Self::TABLE_HEADER)
                .saturating_add(rows(agents)),
            1_u16
                .saturating_add(Self::TABLE_HEADER)
                .saturating_add(rows(plugins)),
        ];
        let mut sections = [SectionRect { y: 0, height: 0 }; 4];
        let mut y = 0;
        for (section, height) in SettingsSection::ALL.into_iter().zip(heights) {
            sections[section.index()] = SectionRect { y, height };
            y = y.saturating_add(height).saturating_add(Self::GAP);
        }
        let content = width.saturating_sub(Self::MARGIN.saturating_mul(2));
        let label_width = if content >= 22 {
            (content / 3).clamp(14, 24)
        } else {
            content.saturating_sub(8)
        };
        let value_x = Self::MARGIN.saturating_add(label_width).saturating_add(2);
        Self {
            sections,
            pref_rows_y: sections[0].y.saturating_add(1),
            plugin_rows_y: sections[3]
                .y
                .saturating_add(1)
                .saturating_add(Self::TABLE_HEADER),
            total_height: y.saturating_add(Self::MARGIN),
            label_width,
            value_x,
        }
    }

    pub(crate) fn section(&self, section: SettingsSection) -> SectionRect {
        self.sections[section.index()]
    }

    pub(crate) fn total_height(&self) -> u16 {
        self.total_height
    }

    pub(crate) fn label_width(&self) -> u16 {
        self.label_width
    }

    pub(crate) fn value_x(&self) -> u16 {
        self.value_x
    }

    pub(crate) fn plugin_rows_y(&self) -> u16 {
        self.plugin_rows_y
    }

    pub(crate) fn preference_row(&self, row: u16) -> Option<usize> {
        let section = self.section(SettingsSection::Preferences);
        let offset = row.checked_sub(self.pref_rows_y)?;
        (usize::from(offset) < usize::from(section.height.saturating_sub(1)))
            .then_some(usize::from(offset))
    }

    pub(crate) fn plugin_row(&self, row: u16) -> Option<usize> {
        let section = self.section(SettingsSection::Plugins);
        let offset = row.checked_sub(self.plugin_rows_y)?;
        (usize::from(offset) < usize::from(section.height.saturating_sub(3)))
            .then_some(usize::from(offset))
    }
}

fn rows(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
}

#[cfg(test)]
#[path = "settings_layout_tests.rs"]
mod tests;
