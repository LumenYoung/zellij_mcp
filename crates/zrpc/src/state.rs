use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zellij_tile::prelude::*;

#[derive(Default)]
pub struct PluginState {
    pub panes: HashMap<String, PaneEntry>,
    pub tabs: Vec<TabEntry>,
    pub current_client_pane_id: Option<PaneId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneEntry {
    pub numeric_id: u32,
    pub is_plugin: bool,
    pub title: String,
    pub command: Option<String>,
    pub tab_index: usize,
    pub tab_name: String,
    pub focused: bool,
    pub floating: bool,
    pub suppressed: bool,
    pub rows: usize,
    pub cols: usize,
}

impl PaneEntry {
    pub fn id_string(&self) -> String {
        if self.is_plugin {
            format!("plugin:{}", self.numeric_id)
        } else {
            format!("terminal:{}", self.numeric_id)
        }
    }

    pub fn pane_id(&self) -> PaneId {
        if self.is_plugin {
            PaneId::Plugin(self.numeric_id)
        } else {
            PaneId::Terminal(self.numeric_id)
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TabEntry {
    pub index: usize,
    pub name: String,
    pub active: bool,
}

impl PluginState {
    pub fn update_panes(&mut self, manifest: PaneManifest) {
        self.panes.clear();

        for (tab_index, panes) in manifest.panes {
            let tab_name = self
                .tabs
                .get(tab_index)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| format!("Tab {}", tab_index));

            for pane in panes {
                let entry = PaneEntry {
                    numeric_id: pane.id,
                    is_plugin: pane.is_plugin,
                    title: pane.title.clone(),
                    command: pane.terminal_command.clone(),
                    tab_index,
                    tab_name: tab_name.clone(),
                    focused: pane.is_focused,
                    floating: pane.is_floating,
                    suppressed: pane.is_suppressed,
                    rows: pane.pane_content_rows,
                    cols: pane.pane_content_columns,
                };
                let key = entry.id_string();
                self.panes.insert(key, entry);
            }
        }
    }

    pub fn update_tabs(&mut self, tabs: Vec<TabInfo>) {
        let max_position = tabs.iter().map(|t| t.position).max().unwrap_or(0);
        let mut entries: Vec<Option<TabEntry>> = vec![None; max_position.saturating_add(1)];

        for tab in tabs {
            entries[tab.position] = Some(TabEntry {
                index: tab.position,
                name: tab.name,
                active: tab.active,
            });
        }

        self.tabs = entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                entry.unwrap_or(TabEntry {
                    index,
                    name: format!("Tab {}", index),
                    active: false,
                })
            })
            .collect();
    }

    pub fn update_clients(&mut self, clients: Vec<ClientInfo>) {
        if clients.is_empty() {
            self.current_client_pane_id = None;
            return;
        }

        self.current_client_pane_id = clients
            .iter()
            .find(|c| c.is_current_client)
            .or_else(|| clients.iter().min_by_key(|c| c.client_id))
            .map(|c| c.pane_id);
    }

    pub fn active_tab_index(&self) -> Option<usize> {
        self.tabs.iter().find(|t| t.active).map(|t| t.index)
    }

    pub fn list_panes(&self, focused_id: Option<&str>) -> Vec<PaneListItem> {
        self.panes
            .values()
            .map(|p| {
                let id = p.id_string();
                PaneListItem {
                    focused: focused_id == Some(id.as_str()),
                    id,
                    pane_type: if p.is_plugin { "plugin" } else { "terminal" }.to_string(),
                    title: p.title.clone(),
                    command: p.command.clone(),
                    tab_index: p.tab_index,
                    tab_name: p.tab_name.clone(),
                    floating: p.floating,
                    suppressed: p.suppressed,
                    rows: p.rows,
                    cols: p.cols,
                }
            })
            .collect()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaneListItem {
    pub id: String,
    pub pane_type: String,
    pub title: String,
    pub command: Option<String>,
    pub tab_index: usize,
    pub tab_name: String,
    pub focused: bool,
    pub floating: bool,
    pub suppressed: bool,
    pub rows: usize,
    pub cols: usize,
}
