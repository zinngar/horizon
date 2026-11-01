//! The Mods tab's "From Java Edition" panel, for a Horizon instance: convert a folder of Java mods,
//! a Java world, or a whole instance another launcher keeps, on this computer
//! (`horizon_launcher::conversions`). A world picked in a game folder's `saves/` takes that game
//! folder's `mods/` along, as the world was played with them.
use super::*;
use horizon_launcher::conversions::{self, Done};
use horizon_launcher::other_launchers::{self, Found};
use horizon_launcher::sources::{Folders, Os};
use std::path::{Path, PathBuf};

/// Lines of the conversion's log the panel shows while it works.
const LOG_SHOWN: usize = 6;

#[derive(Default)]
pub(super) struct Convert {
    pub(super) busy: bool,
    pub(super) log: VecDeque<String>,
    pub(super) done: Option<Result<Done, String>>,
    /// Other launchers' instances, read once in the background.
    others: Option<Vec<Found>>,
    scanning: bool,
}

/// The mods folder a world was played with: `<game>/mods` beside `<game>/saves/<world>`.
fn mods_beside(world: &Path) -> Option<PathBuf> {
    let saves = world
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "saves"))?;
    let mods = saves.parent()?.join("mods");
    mods.is_dir().then_some(mods)
}

impl Launcher {
    /// Runs a conversion into `home` on a thread; its lines and its end arrive as messages.
    fn convert_job(
        &mut self,
        home: PathBuf,
        job: impl FnOnce(&Path, &dyn Fn(String)) -> Result<Done, String> + Send + 'static,
    ) {
        if self.convert.busy {
            return;
        }
        self.convert.busy = true;
        self.convert.log.clear();
        self.convert.done = None;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let lines = tx.clone();
            let log = move |line: String| {
                let _ = lines.send(Msg::ConvertLine(line));
            };
            let result = job(&home, &log);
            let _ = tx.send(Msg::ConvertDone(result.map(Box::new)));
        });
    }

    pub(super) fn convert_line(&mut self, line: String) {
        self.convert.log.push_back(line);
        if self.convert.log.len() > LOG_SHOWN {
            self.convert.log.pop_front();
        }
    }

    pub(super) fn convert_done(&mut self, result: Result<Box<Done>, String>) {
        self.convert.busy = false;
        self.convert.done = Some(result.map(|d| *d));
        // The worlds list shows the new worlds.
        self.worlds.stale();
    }

    /// Asks for a folder, then converts it into `home`.
    fn convert_pick(&mut self, world: bool, home: PathBuf, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr("Convert").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await {
                if let Some(path) = paths.pop() {
                    let _ = this.update(cx, |this, cx| {
                        if world {
                            let mods = mods_beside(&path);
                            this.convert_job(home, move |home, log| {
                                conversions::world(&path, mods.as_deref(), home, log)
                            });
                        } else {
                            this.convert_job(home, move |home, log| {
                                conversions::mods(&path, home, log)
                            });
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Reads the other launchers' instances once, in the background.
    fn scan_others(&mut self, cx: &mut Context<Self>) {
        if self.convert.others.is_some() || self.convert.scanning {
            return;
        }
        self.convert.scanning = true;
        let task = cx.background_executor().spawn(async move {
            Folders::current()
                .map(|f| other_launchers::scan(Os::current(), &f))
                .unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                this.convert.others = Some(found);
                this.convert.scanning = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// The panel, for the Horizon instance whose game folder is `home`.
    pub(super) fn java_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let home = self.mods.home();
        self.scan_others(cx);
        let busy = self.convert.busy;
        let pick = |id: &'static str, label: &'static str, world: bool, home: PathBuf| {
            let b = button(id, label, Kind::Secondary, &t);
            if busy {
                disabled(b)
            } else {
                b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.convert_pick(world, home.clone(), cx);
                    cx.notify();
                }))
            }
        };
        let mut panel = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(caps(tr("From Java Edition")).text_color(t.muted))
            .child(text(theme::CAPTION).text_color(t.muted).child(tr(
                "Bring your Java mods and worlds to Horizon. Blocks, items, recipes and world \
                 generation carry over; what a mod does in Java code doesn't. A world's modded \
                 blocks its mods don't bring become vanilla ones. Nothing is changed where they \
                 come from.",
            )))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(pick(
                        "convert-mods",
                        "Convert a mods folder…",
                        false,
                        home.clone(),
                    ))
                    .child(pick(
                        "convert-world",
                        "Convert a Java world…",
                        true,
                        home.clone(),
                    )),
            );
        if busy || !self.convert.log.is_empty() {
            let start = self.convert.log.len().saturating_sub(LOG_SHOWN);
            panel = panel.child(
                plate(&t)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(16.))
                    .child(
                        caps(tr(if busy { "Working" } else { "Done" })).text_color(if busy {
                            t.accent
                        } else {
                            t.muted
                        }),
                    )
                    .children(self.convert.log.iter().skip(start).map(|line| {
                        text(theme::CAPTION)
                            .font_family(theme::MONO)
                            .text_color(t.ink_soft)
                            .truncate()
                            .child(line.clone())
                    })),
            );
        }
        match &self.convert.done {
            Some(Ok(done)) => {
                let work = done.work.clone();
                let mut summary = div().flex().flex_col().gap(px(4.)).child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(text(theme::UI).text_color(t.ink).child(done.summary()))
                        .child(
                            button("convert-reports", "Open reports", Kind::Ghost, &t)
                                .on_click(move |_, _, cx| cx.open_with_system(&work)),
                        ),
                );
                for line in done.notes.iter().take(LOG_SHOWN) {
                    summary = summary.child(
                        text(theme::CAPTION)
                            .text_color(t.muted)
                            .truncate()
                            .child(line.clone()),
                    );
                }
                panel = panel.child(summary);
            }
            Some(Err(e)) => panel = panel.child(notice(e.clone(), &t)),
            None => {}
        }
        let others = self.convert.others.clone().unwrap_or_default();
        if !others.is_empty() {
            let mut list = div().flex().flex_col();
            for (i, found) in others.into_iter().enumerate() {
                let name = found.name.clone();
                let plays = match (&found.minecraft, &found.loader) {
                    (Some(v), Some(l)) => format!("{} · {v} · {}", found.launcher, l.name),
                    (Some(v), None) => format!("{} · {v}", found.launcher),
                    _ => found.launcher.to_string(),
                };
                let mut bring = button(
                    format!("bring-{i}"),
                    "Bring to Horizon",
                    Kind::Secondary,
                    &t,
                );
                bring = if busy {
                    disabled(bring)
                } else {
                    let home = home.clone();
                    bring.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let found = found.clone();
                        this.convert_job(home.clone(), move |home, log| {
                            conversions::instance(&found, home, log)
                        });
                        cx.notify();
                    }))
                };
                list = list.child(index_row(
                    i + 1,
                    name,
                    Some(
                        text(theme::CAPTION)
                            .text_color(t.muted)
                            .child(plays)
                            .into_any_element(),
                    ),
                    Some(bring.into_any_element()),
                    &t,
                ));
            }
            panel = panel
                .child(caps(tr("In your other launchers")).text_color(t.muted))
                .child(list);
        }
        panel
    }
}
