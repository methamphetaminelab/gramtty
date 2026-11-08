use crate::input::TextInput;
use crate::model::Attachment;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Sticker,
    Animation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    pub file_id: i32,
    pub emoji: String,
    pub width: i32,
    pub height: i32,
    pub duration: i32,
    pub att: Attachment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Recent,
    Favorite,
    Gifs,
    Set(i64),
}

#[derive(Debug, Default)]
pub struct SetEntry {
    pub id: i64,
    pub title: String,
    pub items: Option<Vec<Item>>,
    pub loading: bool,
}

#[derive(Debug, Default)]
pub struct Library {
    pub recent: Option<Vec<Item>>,
    pub favorite: Option<Vec<Item>>,
    pub gifs: Option<Vec<Item>>,
    pub sets: Vec<SetEntry>,
    pub sets_loaded: bool,
}

pub enum Data {
    Recent(Vec<Item>),
    Favorite(Vec<Item>),
    Gifs(Vec<Item>),
    Sets(Vec<(i64, String)>),
    Set(i64, Vec<Item>),
    Search(String, Vec<Item>),
}

impl Library {
    pub fn tabs(&self) -> Vec<(Tab, String)> {
        let mut out = vec![
            (Tab::Recent, "Recent".to_owned()),
            (Tab::Favorite, "Favorites".to_owned()),
            (Tab::Gifs, "GIFs".to_owned()),
        ];
        out.extend(self.sets.iter().map(|s| (Tab::Set(s.id), s.title.clone())));
        out
    }

    pub fn items(&self, tab: Tab) -> Option<&[Item]> {
        match tab {
            Tab::Recent => self.recent.as_deref(),
            Tab::Favorite => self.favorite.as_deref(),
            Tab::Gifs => self.gifs.as_deref(),
            Tab::Set(id) => self.sets.iter().find(|s| s.id == id)?.items.as_deref(),
        }
    }

    pub fn apply(&mut self, data: Data) -> Option<(String, Vec<Item>)> {
        match data {
            Data::Recent(v) => self.recent = Some(v),
            Data::Favorite(v) => self.favorite = Some(v),
            Data::Gifs(v) => self.gifs = Some(v),
            Data::Sets(sets) => {
                let mut old = std::mem::take(&mut self.sets);
                self.sets = sets
                    .into_iter()
                    .map(|(id, title)| match old.iter().position(|s| s.id == id) {
                        Some(i) => {
                            let mut s = old.swap_remove(i);
                            s.title = title;
                            s
                        }
                        None => SetEntry {
                            id,
                            title,
                            ..SetEntry::default()
                        },
                    })
                    .collect();
                self.sets_loaded = true;
            }
            Data::Set(id, items) => {
                if let Some(s) = self.sets.iter_mut().find(|s| s.id == id) {
                    s.items = Some(items);
                    s.loading = false;
                }
            }
            Data::Search(q, items) => return Some((q, items)),
        }
        None
    }

    fn all_stickers(&self) -> impl Iterator<Item = &Item> {
        self.recent
            .iter()
            .chain(self.favorite.iter())
            .flatten()
            .chain(self.sets.iter().filter_map(|s| s.items.as_ref()).flatten())
    }
}

pub fn emoji_matches(item: &Item, query: &str) -> bool {
    let q = query.trim();
    !q.is_empty() && item.emoji.contains(q)
}

pub fn search_results(lib: &Library, query: &str, remote: &[Item]) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    let mut push = |it: &Item| {
        if !out.iter().any(|o| o.file_id == it.file_id) {
            out.push(it.clone());
        }
    };
    for it in lib.all_stickers().filter(|it| emoji_matches(it, query)) {
        push(it);
    }
    for it in remote {
        push(it);
    }
    out
}

pub struct Picker {
    pub tab: usize,
    pub sel: usize,
    pub query: TextInput,
    pub remote: Vec<Item>,
    pub remote_for: String,
    pub searching: bool,
    pub scroll: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Picker {
    pub fn new(tab: usize) -> Self {
        Self {
            tab,
            sel: 0,
            query: TextInput::default(),
            remote: Vec::new(),
            remote_for: String::new(),
            searching: false,
            scroll: 0,
            cols: 1,
            rows: 1,
        }
    }

    pub fn current_tab(&self, lib: &Library) -> Tab {
        let tabs = lib.tabs();
        tabs.get(self.tab.min(tabs.len() - 1))
            .map_or(Tab::Recent, |(t, _)| *t)
    }

    pub fn visible(&self, lib: &Library) -> Vec<Item> {
        let q = self.query.text().trim();
        if !q.is_empty() {
            let remote: &[Item] = if self.remote_for == q {
                &self.remote
            } else {
                &[]
            };
            return search_results(lib, q, remote);
        }
        lib.items(self.current_tab(lib))
            .map(<[Item]>::to_vec)
            .unwrap_or_default()
    }

    pub fn switch_tab(&mut self, lib: &Library, delta: isize) {
        let n = lib.tabs().len() as isize;
        self.tab = (self.tab as isize + delta).rem_euclid(n) as usize;
        self.sel = 0;
        self.scroll = 0;
    }

    pub fn move_sel(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.sel = 0;
            return;
        }
        self.sel = (self.sel as isize + delta).clamp(0, len as isize - 1) as usize;
    }

    pub fn fix_scroll(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let row = self.sel / cols;
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + rows {
            self.scroll = row + 1 - rows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileRef, MediaKind};

    fn item(id: i32, emoji: &str) -> Item {
        Item {
            kind: ItemKind::Sticker,
            file_id: id,
            emoji: emoji.to_owned(),
            width: 512,
            height: 512,
            duration: 0,
            att: Attachment::new(
                MediaKind::Sticker,
                FileRef {
                    id,
                    ..FileRef::default()
                },
            ),
        }
    }

    fn lib() -> Library {
        let mut lib = Library::default();
        lib.apply(Data::Recent(vec![item(1, "😀"), item(2, "🐱")]));
        lib.apply(Data::Favorite(vec![item(2, "🐱")]));
        lib.apply(Data::Sets(vec![(7, "Cats".into()), (8, "Dogs".into())]));
        lib.apply(Data::Set(7, vec![item(3, "🐱"), item(4, "😺")]));
        lib
    }

    #[test]
    fn tabs_and_items() {
        let lib = lib();
        let tabs: Vec<String> = lib.tabs().into_iter().map(|(_, t)| t).collect();
        assert_eq!(tabs, ["Recent", "Favorites", "GIFs", "Cats", "Dogs"]);
        assert_eq!(lib.items(Tab::Set(7)).map(<[Item]>::len), Some(2));
        assert!(lib.items(Tab::Set(8)).is_none());
        assert!(lib.items(Tab::Gifs).is_none());
        let mut lib = lib;
        lib.apply(Data::Sets(vec![(8, "Dogs!".into()), (7, "Cats".into())]));
        assert_eq!(lib.sets[0].title, "Dogs!");
        assert_eq!(lib.items(Tab::Set(7)).map(<[Item]>::len), Some(2));
        assert!(lib.apply(Data::Search("x".into(), vec![])).is_some());
    }

    #[test]
    fn search_merges_local_and_remote_without_duplicates() {
        let lib = lib();
        let ids = |v: Vec<Item>| v.into_iter().map(|i| i.file_id).collect::<Vec<_>>();
        assert_eq!(ids(search_results(&lib, "🐱", &[])), vec![2, 3]);
        assert_eq!(
            ids(search_results(&lib, "🐱", &[item(3, "🐱"), item(9, "🐱")])),
            vec![2, 3, 9]
        );
        assert!(search_results(&lib, " ", &[]).is_empty());
        let mut p = Picker::new(3);
        assert_eq!(p.current_tab(&lib), Tab::Set(7));
        assert_eq!(ids(p.visible(&lib)), vec![3, 4]);
        p.query = TextInput::with_text("😺");
        assert_eq!(ids(p.visible(&lib)), vec![4]);
        p.remote = vec![item(10, "😺")];
        p.remote_for = "other".into();
        assert_eq!(ids(p.visible(&lib)), vec![4]);
        p.remote_for = "😺".into();
        assert_eq!(ids(p.visible(&lib)), vec![4, 10]);
    }

    #[test]
    fn navigation_wraps_tabs_and_scrolls_grid() {
        let lib = lib();
        let mut p = Picker::new(0);
        p.switch_tab(&lib, -1);
        assert_eq!(p.current_tab(&lib), Tab::Set(8));
        p.switch_tab(&lib, 1);
        assert_eq!(p.current_tab(&lib), Tab::Recent);
        p.move_sel(25, 30);
        assert_eq!(p.sel, 25);
        p.fix_scroll(4, 3);
        assert_eq!(p.scroll, 4);
        p.move_sel(-100, 30);
        p.fix_scroll(4, 3);
        assert_eq!((p.sel, p.scroll), (0, 0));
        p.move_sel(5, 0);
        assert_eq!(p.sel, 0);
    }
}
