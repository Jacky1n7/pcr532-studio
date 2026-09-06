use crate::{
    document::{self, Card, Document},
    ndef,
    tasks::{self, Event, Job, Operation},
};
use anyhow::{Context, Result, ensure};
use eframe::egui::{self, Color32, RichText, TextEdit};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

const PAGES: [&str; 7] = [
    "工作台",
    "IC 数据",
    "密钥与恢复",
    "NTAG / NDEF",
    "工具",
    "本地记录",
    "迁移状态",
];
pub struct App {
    smoke_requested: bool,
    page: usize,
    sidebar: bool,
    ports: Vec<String>,
    port: String,
    baud: u32,
    card: Option<Card>,
    document: Document,
    capacity: usize,
    keys: String,
    recovery_source: usize,
    recovery_target: usize,
    recovery_key: String,
    recovery_b: bool,
    selected: BTreeSet<usize>,
    trailers: bool,
    edit_block: usize,
    edit_hex: String,
    logs: Vec<String>,
    status: String,
    receiver: Option<mpsc::Receiver<Event>>,
    thread: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    pending: Option<(Job, String)>,
    ntag_pages: usize,
    ntag_password: String,
    ntag_read: String,
    ntag_write: String,
    ntag_start: usize,
    ndef_kind: usize,
    ndef_text: String,
    ndef_extra: String,
    tool_output: String,
    value: i32,
    address: u8,
    ui_error: Option<String>,
    archive: Vec<std::path::PathBuf>,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        style.spacing.item_spacing = egui::vec2(10., 9.);
        style.spacing.button_padding = egui::vec2(14., 8.);
        style.visuals.panel_fill = Color32::from_rgb(16, 23, 33);
        style.visuals.selection.bg_fill = Color32::from_rgb(32, 108, 106);
        style.visuals.override_text_color = Some(Color32::from_rgb(229, 235, 239));
        style.spacing.interact_size.y = 30.;
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(23.));
        ctx.set_style_of(egui::Theme::Dark, style);
        let mut fonts = egui::FontDefinitions::default();
        for path in [
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ] {
            if let Ok(data) = std::fs::read(path) {
                fonts.font_data.insert(
                    "system-cjk".into(),
                    Arc::new(egui::FontData::from_owned(data)),
                );
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("system-cjk".into());
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("system-cjk".into());
                break;
            }
        }
        ctx.set_fonts(fonts);
        let mut app = Self {
            smoke_requested: false,
            page: 0,
            sidebar: true,
            ports: vec![],
            port: String::new(),
            baud: 115200,
            card: None,
            document: Document::empty(64).expect("valid capacity"),
            capacity: 64,
            keys: "FFFFFFFFFFFF\nA0A1A2A3A4A5\nD3F7D3F7D3F7".into(),
            recovery_source: 0,
            recovery_target: 4,
            recovery_key: "FFFFFFFFFFFF".into(),
            recovery_b: false,
            selected: BTreeSet::new(),
            trailers: false,
            edit_block: 0,
            edit_hex: String::new(),
            logs: vec![],
            status: "就绪".into(),
            receiver: None,
            thread: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending: None,
            ntag_pages: 45,
            ntag_password: String::new(),
            ntag_read: String::new(),
            ntag_write: String::new(),
            ntag_start: 4,
            ndef_kind: 0,
            ndef_text: String::new(),
            ndef_extra: String::new(),
            tool_output: String::new(),
            value: 0,
            address: 1,
            ui_error: None,
            archive: vec![],
        };
        app.refresh_ports();
        app.refresh_archive();
        app
    }
    fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.port_name)
            .filter(|p| !cfg!(target_os = "macos") || p.starts_with("/dev/cu."))
            .collect();
        if !self.ports.contains(&self.port) {
            self.port = self
                .ports
                .iter()
                .find(|p| p.contains("usbserial"))
                .or(self.ports.first())
                .cloned()
                .unwrap_or_default();
        }
    }
    fn refresh_archive(&mut self) {
        self.archive = std::fs::read_dir(tasks::data_dir())
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|s| s == "json"))
            .collect();
        self.archive.sort();
        self.archive.reverse();
    }
    fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    fn log(&mut self, message: String) {
        self.logs.push(message);
        if self.logs.len() > 2000 {
            self.logs.drain(..500);
        }
    }
    fn error(&mut self, e: impl std::fmt::Display) {
        self.ui_error = Some(e.to_string());
    }
    fn job(&self, operation: Operation) -> Job {
        Job {
            port: self.port.clone(),
            baud: self.baud,
            operation,
        }
    }
    fn start(&mut self, job: Job) {
        if self.busy() {
            self.error("已有任务运行");
            return;
        }
        if job.port.is_empty() {
            self.error("请先选择串口");
            return;
        }
        if matches!(job.operation, Operation::Scan) {
            self.card = None;
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.status = "执行中".into();
        self.log(format!("开始任务 · {}", job.port));
        self.receiver = Some(rx);
        self.thread = Some(std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tasks::run(job, cancel, &tx)
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("后台任务异常终止")));
            let _ = tx.send(Event::Done(result));
        }));
    }
    fn poll(&mut self) {
        let events = self
            .receiver
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Log(m) => self.log(m),
                Event::Card(c) => {
                    self.log(format!("UID {} · ATQA {} · SAK {}", c.uid, c.atqa, c.sak));
                    self.card = Some(c);
                }
                Event::Document(d) => {
                    if let (Some(uid), Some(atqa), Some(sak)) =
                        (d.card.get("uid"), d.card.get("atqa"), d.card.get("sak"))
                    {
                        self.card = Some(Card {
                            uid: uid.clone(),
                            atqa: atqa.clone(),
                            sak: sak.clone(),
                        });
                    }
                    self.document = d;
                    self.selected.clear();
                    self.capacity = self.document.blocks.len();
                    self.page = 1;
                    self.refresh_archive();
                }
                Event::Ntag(c, data) => {
                    self.card = Some(c);
                    self.ntag_read = hex::encode_upper(data);
                }
                Event::Done(result) => {
                    self.status = match result {
                        Ok(()) => "完成".into(),
                        Err(e) => format!("停止或失败：{e:#}"),
                    };
                    self.log(self.status.clone());
                    self.receiver = None;
                    if let Some(t) = self.thread.take() {
                        let _ = t.join();
                    }
                }
            }
        }
    }
    fn read(&mut self) {
        match document::parse_keys(&self.keys) {
            Ok(keys) => self.start(self.job(Operation::Read {
                keys,
                blocks: self.capacity,
            })),
            Err(e) => self.error(e),
        }
    }
    fn write(&mut self, all: bool) -> Result<()> {
        let uid = self.card.as_ref().context("请先识别目标卡")?.uid.clone();
        let keys = document::parse_keys(&self.keys)?;
        let indices: Vec<_> = if all {
            (1..self.document.blocks.len()).collect::<Vec<_>>()
        } else {
            self.selected.iter().copied().collect()
        }
        .into_iter()
        .filter(|b| self.trailers || !document::is_trailer(*b))
        .collect();
        crate::pn532::validate_write(
            &self.document,
            &indices,
            crate::pn532::capacity(self.card.as_ref().unwrap())?,
            self.trailers,
        )?;
        let summary = format!(
            "目标 UID：{uid}\n串口：{}\n写入块：{indices:?}\n包含密钥/访问位：{}\n写入会覆盖目标卡。每块写入后回读校验；中途停止可能已有部分块完成。",
            self.port, self.trailers
        );
        self.pending = Some((
            self.job(Operation::Write {
                document: self.document.clone(),
                indices,
                uid,
                keys,
                trailers: self.trailers,
            }),
            summary,
        ));
        Ok(())
    }
    fn import(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("卡片备份", &["json", "mfd", "bin", "mct", "eml", "dump"])
            .pick_file()
        {
            match Document::load(&p) {
                Ok(d) => {
                    self.document = d;
                    self.capacity = self.document.blocks.len();
                    self.selected.clear();
                }
                Err(e) => self.error(e),
            }
        }
    }
    fn export(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_file_name("card.json")
            .add_filter("JSON", &["json"])
            .add_filter("MFD", &["mfd"])
            .add_filter("MCT", &["mct"])
            .add_filter("EML", &["eml"])
            .save_file()
            && let Err(e) = self.document.save(&p)
        {
            self.error(e);
        }
    }
    fn memory(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("capacity")
                .selected_text(format!("{} 块", self.capacity))
                .show_ui(ui, |ui| {
                    for n in [20, 64, 128, 256] {
                        ui.selectable_value(&mut self.capacity, n, format!("{n} 块"));
                    }
                });
            if ui.button("读取卡片").clicked() {
                self.read();
            }
            if ui.button("导入").clicked() {
                self.import();
            }
            if ui.button("导出").clicked() {
                self.export();
            }
            if ui.button("新建空白文档").clicked()
                && rfd::MessageDialog::new()
                    .set_title("替换编辑区")
                    .set_description("未保存的修改会丢失；仅生成数据，不向卡片写入。")
                    .set_buttons(rfd::MessageButtons::OkCancel)
                    .show()
                    == rfd::MessageDialogResult::Ok
            {
                self.document = Document::blank(self.capacity).unwrap();
                self.selected.clear();
            }
        });
        ui.label(format!(
            "已知 {}/{} 块 · 选中 {} 块",
            self.document.known(),
            self.document.blocks.len(),
            self.selected.len()
        ));
        egui::ScrollArea::vertical()
            .id_salt("memory")
            .max_height(330.)
            .show(ui, |ui| {
                egui::Grid::new("blocks")
                    .striped(true)
                    .min_col_width(45.)
                    .show(ui, |ui| {
                        for title in ["选择", "扇区", "块", "HEX · 点击后在下方编辑", "状态"]
                        {
                            ui.strong(title);
                        }
                        ui.end_row();
                        for b in 0..self.document.blocks.len() {
                            let mut selected = self.selected.contains(&b);
                            if ui.checkbox(&mut selected, "").changed() {
                                if selected {
                                    self.selected.insert(b);
                                } else {
                                    self.selected.remove(&b);
                                }
                            }
                            ui.label(document::sector_of(b).unwrap().to_string());
                            ui.label(b.to_string());
                            let value = self.document.blocks[b].as_deref().unwrap_or("— 未读取 —");
                            if ui
                                .selectable_label(
                                    self.edit_block == b,
                                    RichText::new(value).monospace().color(
                                        if document::is_trailer(b) {
                                            Color32::from_rgb(90, 209, 187)
                                        } else {
                                            Color32::LIGHT_GRAY
                                        },
                                    ),
                                )
                                .clicked()
                            {
                                self.edit_block = b;
                                self.edit_hex = self.document.blocks[b].clone().unwrap_or_default();
                            }
                            ui.label(if self.document.blocks[b].is_none() {
                                "未知"
                            } else if document::is_trailer(b) {
                                "密钥 / 访问位"
                            } else {
                                "数据"
                            });
                            ui.end_row();
                        }
                    });
            });
        ui.horizontal(|ui| {
            ui.label(format!("编辑块 {}", self.edit_block));
            ui.add(
                TextEdit::singleline(&mut self.edit_hex)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(400.),
            );
            if ui.button("应用编辑").clicked()
                && let Err(e) = self.document.set_block(self.edit_block, &self.edit_hex)
            {
                self.error(e);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.trailers, "写入密钥 / 访问位");
            if ui.button("写入选中块").clicked()
                && let Err(e) = self.write(false)
            {
                self.error(e);
            }
            if ui.button("写入全部用户块").clicked()
                && let Err(e) = self.write(true)
            {
                self.error(e);
            }
        });
    }
    fn keys(&mut self, ui: &mut egui::Ui) {
        ui.label("每行一个 12 位 HEX 密钥。读取和写入认证使用这份字典。");
        ui.add(
            TextEdit::multiline(&mut self.keys)
                .font(egui::TextStyle::Monospace)
                .desired_rows(10)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui.button("字典读取").clicked() {
                self.read();
            }
            if ui.button("导入字典").clicked()
                && let Some(p) = rfd::FileDialog::new().pick_file()
            {
                match std::fs::read_to_string(p)
                    .map_err(anyhow::Error::from)
                    .and_then(|s| document::parse_keys(&s))
                {
                    Ok(k) => self.keys = k.join("\n"),
                    Err(e) => self.error(e),
                }
            }
            if ui.button("导出字典").clicked()
                && let Some(p) = rfd::FileDialog::new().set_file_name("keys.txt").save_file()
                && let Err(e) = document::parse_keys(&self.keys)
                    .and_then(|keys| document::atomic_write(&p, keys.join("\n").as_bytes()))
            {
                self.error(e);
            }
            if ui.button("提取备份密钥").clicked() {
                self.keys = self
                    .document
                    .keys
                    .values()
                    .flat_map(|k| k.values().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        });
        ui.separator();
        ui.add_space(8.);
        ui.strong("固定加密随机数卡");
        ui.label("Fudan 1K 兼容卡可尝试本地诊断读取。读取所有数据块，不修改卡片；普通 A/B 密钥需单独恢复。");
        if ui.button("Fudan 本地完整读取").clicked() {
            self.start(self.job(Operation::FudanRead));
        }
        if ui.button("Fudan 恢复普通密钥并读取").clicked() {
            self.start(self.job(Operation::FudanRecover));
        }
        ui.weak("通过固定随机数与跨扇区密钥复用筛选候选，只有通过普通认证的密钥才会写入备份。");
        ui.add_space(12.);
        ui.label("本地 nested 恢复（实验）：使用一个已知 A 密钥，恢复指定目标块的 A/B 密钥。只执行认证和读取。");
        ui.horizontal(|ui| {
            ui.label("已知 A 密钥块");
            ui.add(egui::DragValue::new(&mut self.recovery_source).range(0..=255));
            ui.add(
                TextEdit::singleline(&mut self.recovery_key)
                    .password(true)
                    .desired_width(150.),
            );
            ui.label("目标块");
            ui.add(egui::DragValue::new(&mut self.recovery_target).range(0..=255));
            ui.checkbox(&mut self.recovery_b, "恢复 Key B");
        });
        ui.horizontal(|ui| {
            let kind = if self.recovery_b { "B" } else { "A" }.to_owned();
            if ui.button("随机数诊断").clicked() {
                self.start(self.job(Operation::Diagnose {
                    block: self.recovery_target,
                    kind: kind.clone(),
                }));
            }
            if ui.button("尝试 nested 并重读").clicked() {
                match document::parse_keys(&self.keys) {
                    Ok(keys) => self.start(self.job(Operation::Nested {
                        source: self.recovery_source,
                        known: self.recovery_key.clone(),
                        target: self.recovery_target,
                        kind,
                        keys,
                    })),
                    Err(e) => self.error(e),
                }
            }
        });
        ui.label(
            "每次搜索最多 240 秒，支持停止；失败只代表此次方法未成功。hardnested / DarkSide 尚未迁移。",
        );
    }
    fn ntag(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("读取页数");
            ui.add(egui::DragValue::new(&mut self.ntag_pages).range(4..=256));
            ui.add(
                TextEdit::singleline(&mut self.ntag_password)
                    .password(true)
                    .hint_text("可选 PWD · 8 位 HEX")
                    .desired_width(200.),
            );
            if ui.button("读取页面").clicked() {
                self.start(self.job(Operation::NtagRead {
                    pages: self.ntag_pages,
                    password: self.ntag_password.clone(),
                }));
            }
            if ui.button("导出读取结果").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .set_file_name("ntag-read.hex")
                    .save_file()
                && let Err(e) = document::atomic_write(&p, self.ntag_read.as_bytes())
            {
                self.error(e);
            }
        });
        ui.label("完整读取结果（只读）");
        ui.add(
            TextEdit::multiline(&mut self.ntag_read)
                .interactive(false)
                .font(egui::TextStyle::Monospace)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.separator();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("ndef_kind")
                .selected_text(ndef::KINDS[self.ndef_kind])
                .show_ui(ui, |ui| {
                    for (i, name) in ndef::KINDS.iter().enumerate() {
                        ui.selectable_value(&mut self.ndef_kind, i, *name);
                    }
                });
            ui.add(
                TextEdit::singleline(&mut self.ndef_text)
                    .hint_text("内容 / SSID / 蓝牙地址")
                    .desired_width(260.),
            );
        });
        if self.ndef_kind == 4 || self.ndef_kind == 5 {
            ui.add(
                TextEdit::singleline(&mut self.ndef_extra)
                    .password(self.ndef_kind == 4)
                    .hint_text(if self.ndef_kind == 4 {
                        "Wi-Fi 密码；空为开放网络"
                    } else {
                        "蓝牙名称"
                    }),
            );
        }
        if ui.button("生成 NDEF 到用户区").clicked() {
            match ndef::generate(
                ndef::KINDS[self.ndef_kind],
                &self.ndef_text,
                &self.ndef_extra,
            )
            .and_then(|m| ndef::tlv(&m))
            {
                Ok(data) => {
                    self.ntag_write = hex::encode_upper(data);
                    self.ntag_start = 4;
                }
                Err(e) => self.error(e),
            }
        }
        ui.add(
            TextEdit::multiline(&mut self.ntag_write)
                .font(egui::TextStyle::Monospace)
                .hint_text("待写用户区 HEX")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.label("起始页");
            ui.add(egui::DragValue::new(&mut self.ntag_start).range(4..=225));
            if ui.button("导入 HEX").clicked()
                && let Some(p) = rfd::FileDialog::new().pick_file()
            {
                match std::fs::read_to_string(p) {
                    Ok(s) => self.ntag_write = s,
                    Err(e) => self.error(e),
                }
            }
            if ui.button("导出 NDEF").clicked() {
                match document::bytes(&self.ntag_write, None).and_then(|d| ndef::from_tlv(&d)) {
                    Ok(data) => {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name("record.ndef")
                            .save_file()
                            && let Err(e) = document::atomic_write(&p, &data)
                        {
                            self.error(e);
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            if ui.button("写入用户区").clicked() {
                let result = (|| -> Result<(Job, String)> {
                    let uid = self.card.as_ref().context("请先识别目标卡")?.uid.clone();
                    let data = document::bytes(&self.ntag_write, None)?;
                    ensure!(!data.is_empty(), "数据为空");
                    let summary = format!(
                        "目标 {uid}\n从页 {} 写入 {} 字节。仅限 NTAG213/215/216 用户区。",
                        self.ntag_start,
                        data.len()
                    );
                    Ok((
                        self.job(Operation::NtagWrite {
                            start: self.ntag_start,
                            data,
                            uid,
                            password: self.ntag_password.clone(),
                        }),
                        summary,
                    ))
                })();
                match result {
                    Ok(p) => self.pending = Some(p),
                    Err(e) => self.error(e),
                }
            }
        });
    }
    fn tools(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (name, hid) in [("读取 HID 扩展", true), ("读取 ID 扩展", false)] {
                if ui.button(name).clicked() {
                    self.start(self.job(Operation::VendorRead { hid }));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("比较备份").clicked()
                && let Some(paths) = rfd::FileDialog::new().pick_files()
            {
                let r = (|| -> Result<String> {
                    ensure!(paths.len() == 2, "请选择两份备份");
                    let a = Document::load(&paths[0])?;
                    let b = Document::load(&paths[1])?;
                    ensure!(a.blocks.len() == b.blocks.len(), "容量不同");
                    Ok(a.blocks
                        .iter()
                        .zip(b.blocks.iter())
                        .enumerate()
                        .filter(|(_, (a, b))| a != b)
                        .map(|(i, (a, b))| {
                            format!(
                                "块 {i}: {} → {}",
                                a.as_deref().unwrap_or("未知"),
                                b.as_deref().unwrap_or("未知")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"))
                })();
                match r {
                    Ok(s) => self.tool_output = if s.is_empty() { "无差异".into() } else { s },
                    Err(e) => self.error(e),
                }
            }
            if ui.button("合并备份").clicked()
                && let Some(paths) = rfd::FileDialog::new().pick_files()
            {
                let result = paths
                    .iter()
                    .map(|p| Document::load(p))
                    .collect::<Result<Vec<_>>>()
                    .and_then(|d| Document::merge(&d));
                match result {
                    Ok(doc) => {
                        if rfd::MessageDialog::new()
                            .set_title("合并成功")
                            .set_description("用合并结果替换编辑区？")
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show()
                            == rfd::MessageDialogResult::Ok
                        {
                            self.document = doc;
                            self.capacity = self.document.blocks.len();
                            self.selected.clear();
                            self.edit_block = 0;
                            self.edit_hex.clear();
                            self.page = 1;
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            if ui.button("检查访问位").clicked() {
                self.tool_output = (0..document::sectors(self.document.blocks.len()).unwrap())
                    .map(|s| {
                        let b = document::sector_blocks(s).unwrap().end - 1;
                        let result = self.document.blocks[b]
                            .as_deref()
                            .context("未知块")
                            .and_then(|s| document::bytes(s, Some(16)))
                            .and_then(|d| document::access(&d));
                        format!("扇区 {s}: {result:?}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            if ui.button("模拟 NDEF Type 4").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("NDEF 记录", &["ndef"])
                    .pick_file()
            {
                match std::fs::read(p).map_err(anyhow::Error::from).and_then(|m| {
                    ndef::validate(&m)?;
                    Ok(m)
                }) {
                    Ok(message) => self.start(self.job(Operation::Emulate { message })),
                    Err(e) => self.error(e),
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("值块");
            ui.add(egui::DragValue::new(&mut self.value));
            ui.label("地址");
            ui.add(egui::DragValue::new(&mut self.address));
            if ui.button("生成").clicked() {
                self.tool_output =
                    hex::encode_upper(document::value_block(self.value, self.address));
            }
            if ui.button("解码编辑块").clicked() {
                match document::bytes(&self.edit_hex, Some(16))
                    .and_then(|d| document::decode_value(&d))
                {
                    Ok((v, a)) => self.tool_output = format!("值 {v}，地址 {a}"),
                    Err(e) => self.error(e),
                }
            }
        });
        ui.add(
            TextEdit::multiline(&mut self.tool_output)
                .font(egui::TextStyle::Monospace)
                .desired_rows(18)
                .desired_width(f32::INFINITY),
        );
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        self.smoke(ui.ctx());
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !self.busy() {
            let modifiers = egui::Modifiers::COMMAND;
            if ui.input_mut(|i| i.consume_key(modifiers, egui::Key::O)) {
                self.import();
            }
            if ui.input_mut(|i| i.consume_key(modifiers, egui::Key::S)) {
                self.export();
            }
            if ui.input_mut(|i| i.consume_key(modifiers, egui::Key::R)) {
                self.start(self.job(Operation::Scan));
            }
        }
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(8.);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("侧栏")
                    .on_hover_text("显示或隐藏导航侧栏")
                    .clicked()
                {
                    self.sidebar = !self.sidebar;
                }
                ui.strong(
                    RichText::new("PCR532 Studio")
                        .size(17.)
                        .color(Color32::from_rgb(88, 205, 186)),
                );
                ui.separator();
                ui.add_enabled_ui(!self.busy(), |ui| {
                    egui::ComboBox::from_id_salt("port")
                        .selected_text(if self.port.is_empty() {
                            "选择串口"
                        } else {
                            &self.port
                        })
                        .width(230.)
                        .show_ui(ui, |ui| {
                            for p in &self.ports {
                                ui.selectable_value(&mut self.port, p.clone(), p);
                            }
                        });
                    egui::ComboBox::from_id_salt("baud")
                        .selected_text(self.baud.to_string())
                        .show_ui(ui, |ui| {
                            for b in [9600, 19200, 38400, 57600, 115200, 230400] {
                                ui.selectable_value(&mut self.baud, b, b.to_string());
                            }
                        });
                    if ui.button("刷新").clicked() {
                        self.refresh_ports();
                    }
                    if ui
                        .add(egui::Button::new("连接 / 识卡").fill(Color32::from_rgb(27, 99, 92)))
                        .on_hover_text("⌘R · 识别卡型与 UID")
                        .clicked()
                    {
                        self.start(self.job(Operation::Scan));
                    }
                });
            });
        });
        if self.sidebar {
            egui::Panel::left("nav")
                .default_size(190.)
                .resizable(true)
                .show(ui, |ui| {
                    ui.add_space(18.);
                    for (i, name) in PAGES.iter().enumerate() {
                        if ui
                            .add_sized(
                                [ui.available_width(), 34.],
                                egui::Button::new(*name)
                                    .selected(self.page == i)
                                    .frame(self.page == i),
                            )
                            .clicked()
                        {
                            self.page = i;
                        }
                        ui.add_space(8.);
                    }
                    ui.separator();
                    ui.small(format!("v{}", env!("CARGO_PKG_VERSION")));
                    if let Some(card) = &self.card {
                        ui.label(format!("UID {}", card.uid));
                        ui.label(format!("ATQA {} / SAK {}", card.atqa, card.sak));
                        ui.weak(crate::pn532::classify(card))
                            .on_hover_text("依据 ATQA/SAK 的只读判断，非精确芯片型号");
                    }
                });
        }
        egui::Panel::bottom("logs")
            .default_size(150.)
            .resizable(true)
            .show(ui, |ui| {
                ui.set_min_height(120.);
                ui.horizontal(|ui| {
                    if self.busy() {
                        ui.spinner();
                    }
                    ui.strong(&self.status);
                    if ui
                        .add_enabled(self.busy(), egui::Button::new("停止任务"))
                        .clicked()
                    {
                        self.cancel.store(true, Ordering::Relaxed);
                        self.log("已请求停止，正在结束当前任务。".into());
                    }
                    if ui.button("保存日志").clicked()
                        && let Some(p) = rfd::FileDialog::new()
                            .set_file_name("pcr532.log")
                            .save_file()
                        && let Err(e) = document::atomic_write(&p, self.logs.join("\n").as_bytes())
                    {
                        self.error(e);
                    }
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.logs.is_empty() {
                            ui.weak("任务记录会显示在这里。所有卡片数据仅保存在本机。");
                        }
                        for line in &self.logs {
                            ui.monospace(line);
                        }
                    });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(14.);
            ui.heading(PAGES[self.page]);
            ui.add_space(18.);
            ui.add_enabled_ui(!self.busy(), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("page")
                    .show(ui, |ui| match self.page {
                        0 => {
                            ui.label("读取、检查和备份你的测试卡。先连接设备，再选择读取方式。");
                            ui.add_space(24.);
                            ui.strong("当前卡片");
                            ui.add_space(8.);
                            if let Some(c) = &self.card {
                                ui.monospace(format!("UID  {}", c.uid));
                                ui.label(format!("ATQA {}    SAK {}", c.atqa, c.sak));
                            } else {
                                ui.weak("尚未识别卡片。将卡放到 IC 感应区后，点击顶部的连接按钮。");
                            }
                            ui.add_space(24.);
                            ui.separator();
                            ui.add_space(16.);
                            ui.strong("开始读取");
                            ui.add_space(8.);
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("使用密钥字典读取").clicked() { self.read(); }
                                if ui.button("Fudan 本地完整读取").clicked() { self.start(self.job(Operation::FudanRead)); }
                                if ui.button("Fudan 恢复密钥并读取").clicked() { self.start(self.job(Operation::FudanRecover)); }
                                if ui.button("打开已有备份…").on_hover_text("⌘O").clicked() { self.import(); self.page = 1; }
                            });
                            ui.add_space(10.);
                            ui.weak("普通卡使用密钥字典；固定加密随机数的 Fudan 兼容卡可尝试本地完整读取。");
                            ui.add_space(24.);
                            ui.strong("数据检查与写入");
                            ui.label("在卡片数据页比较区块、编辑备份并导出。写入前需识别目标卡并确认待写内容。");
                            if ui.button("查看卡片数据").clicked() { self.page = 1; }
                        }
                        1 => self.memory(ui),
                        2 => self.keys(ui),
                        3 => self.ntag(ui),
                        4 => self.tools(ui),
                        5 => {
                            if ui.button("刷新本地备份").clicked() {
                                self.refresh_archive();
                            }
                            let mut chosen = None;
                            for p in &self.archive {
                                if ui
                                    .button(p.file_name().unwrap_or_default().to_string_lossy())
                                    .clicked()
                                {
                                    chosen = Some(p.clone());
                                }
                            }
                            if let Some(p) = chosen {
                                match Document::load(&p) {
                                    Ok(doc) => {
                                        self.document = doc;
                                        self.capacity = self.document.blocks.len();
                                        self.edit_block = 0;
                                        self.edit_hex.clear();
                                        self.selected.clear();
                                        self.page = 1;
                                    }
                                    Err(e) => self.error(e),
                                }
                            }
                        }
                        _ => {
                            ui.label(include_str!("../docs/FEATURES.txt"));
                        }
                    });
            });
        });
        if let Some((job, summary)) = self.pending.clone() {
            egui::Window::new("确认写入")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(summary);
                    ui.horizontal(|ui| {
                        if ui.button("确认执行").clicked() {
                            self.pending = None;
                            self.start(job);
                        }
                        if ui.button("取消").clicked() {
                            self.pending = None;
                        }
                    });
                });
        }
        if let Some(message) = self.ui_error.clone() {
            egui::Window::new("无法完成")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.label(message);
                    if ui.button("关闭").clicked() {
                        self.ui_error = None;
                    }
                });
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub fn run() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240., 850.])
            .with_min_inner_size([1000., 720.]),
        ..Default::default()
    };
    eframe::run_native(
        "PCR532 Studio · Rust",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

impl App {
    // Opt-in renderer smoke test for this application only; never sends card commands.
    fn smoke(&mut self, ctx: &egui::Context) {
        let Some(dir) = std::env::var_os("PCR532_SMOKE_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let images = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        for image in images {
            let result = (|| -> Result<()> {
                std::fs::create_dir_all(&dir)?;
                let f = std::fs::File::create(dir.join(format!("page-{}.png", self.page)))?;
                let mut encoder = png::Encoder::new(f, image.size[0] as u32, image.size[1] as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header()?;
                let pixels: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                writer.write_image_data(&pixels)?;
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("Renderer smoke test: {e}");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            self.smoke_requested = false;
            if self.page == PAGES.len() - 1 {
                println!("Rendered all {} pages successfully", PAGES.len());
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            self.page += 1;
        }
        if !self.smoke_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.smoke_requested = true;
        }
        ctx.request_repaint();
    }
}
