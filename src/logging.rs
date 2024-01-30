static LINES: tokio::sync::RwLock<Vec<(log::Level, String)>> =
    tokio::sync::RwLock::const_new(Vec::new());

struct Log;

impl log::Log for Log {
    fn flush(&self) {}

    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target().starts_with("usd_render")
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let formatted = (record.level(), record.args().to_string());

        tokio::spawn(async move {
            let mut lines = LINES.write().await;
            let to_remove = lines.len().saturating_sub(1000);
            lines.drain(..to_remove);
            lines.push(formatted);
        });
    }
}

pub async fn get_lines() -> Lines {
    Lines {
        lines: LINES.read().await,
    }
}

pub struct Lines {
    lines: tokio::sync::RwLockReadGuard<'static, Vec<(log::Level, String)>>,
}

impl Lines {
    pub fn draw(&self, ui: &mut egui::Ui) {
        egui::containers::scroll_area::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                egui::Grid::new("log").striped(true).show(ui, |ui| {
                    for (level, line) in self.lines.iter() {
                        ui.label(level.to_string());
                        ui.label(line);
                        ui.end_row();
                    }
                })
            });
    }
}

pub fn setup() -> anyhow::Result<()> {
    log::set_logger(&Log)?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}
