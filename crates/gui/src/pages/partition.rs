use crate::app::HaucetApp;
use crate::pages::{Page, run_button};
use crate::util::{human_size, kv, message_box, section};
use common::entropy::EntropySummary;
use common::partition::{CertSummary, HarmonySummary, PartitionSummary};
use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Debug, Default)]
pub struct PartitionPage {
    pub input: String,
    pub summary: Option<PartitionSummary>,
    pub entropy_summary: Option<EntropySummary>,
    pub partition_error: Option<String>,
    pub entropy_error: Option<String>,
    pending_job: Option<PartitionJob>,
}

#[derive(Debug, Clone, Copy)]
enum PartitionJob {
    Info,
    Entropy,
}

impl PartitionPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        egui::ScrollArea::vertical()
            .id_salt("partition-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "选择分区镜像以查看详细信息，也可以计算文件字节分布的 Shannon 信息熵。",
                    )
                    .weak(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("镜像文件").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .hint_text("镜像路径或拖放文件到这里")
                            .desired_width(ui.available_width() - 160.0),
                    );
                    if ui.button("选择文件…").clicked()
                        && let Some(path) = app.pick_file("选择镜像或文件", &[])
                    {
                        self.input = path.display().to_string();
                    }
                });
                let drops = app.take_drops(ui.ctx());
                if let Some(path) = drops.first() {
                    self.input = path.display().to_string();
                }

                ui.add_space(6.0);
                let can_run = !app.job_running() && !self.input.trim().is_empty();
                ui.horizontal(|ui| {
                    if run_button(ui, "查看信息", can_run, None).clicked() {
                        self.summary = None;
                        self.partition_error = None;
                        self.pending_job = Some(PartitionJob::Info);
                        app.start_job(crate::worker::JobOp::PartitionInfo {
                            image: self.input.trim().to_owned(),
                        });
                    }
                    if run_button(ui, "计算信息熵", can_run, None).clicked() {
                        self.entropy_summary = None;
                        self.entropy_error = None;
                        self.pending_job = Some(PartitionJob::Entropy);
                        app.start_job(crate::worker::JobOp::FileEntropy {
                            file: self.input.trim().to_owned(),
                        });
                    }
                });
                ui.add_space(8.0);

                if let Some(error) = &self.partition_error {
                    message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
                }
                if let Some(summary) = &self.summary {
                    self.render(ui, summary);
                }
                ui.add_space(10.0);
                if let Some(error) = &self.entropy_error {
                    message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
                }
                if let Some(summary) = &self.entropy_summary {
                    crate::pages::entropy::render_summary(ui, summary);
                }
            });
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Partition) else {
            return;
        };
        match self.pending_job.take().unwrap_or(PartitionJob::Info) {
            PartitionJob::Info => self.apply_partition_result(result),
            PartitionJob::Entropy => self.apply_entropy_result(result),
        }
    }

    fn apply_partition_result(&mut self, result: crate::job::JobResult) {
        if !result.ok {
            self.summary = None;
            self.partition_error = Some(result.summary);
            return;
        }
        if let Some(payload) = result.payload {
            match serde_json::from_value::<PartitionSummary>(payload) {
                Ok(summary) => {
                    self.summary = Some(summary);
                    self.partition_error = None;
                }
                Err(error) => {
                    self.partition_error = Some(format!("解析结果失败：{error}"));
                }
            }
        }
    }

    fn apply_entropy_result(&mut self, result: crate::job::JobResult) {
        if !result.ok {
            self.entropy_summary = None;
            self.entropy_error = Some(result.summary);
            return;
        }
        if let Some(payload) = result.payload {
            match serde_json::from_value::<EntropySummary>(payload) {
                Ok(summary) => {
                    self.entropy_summary = Some(summary);
                    self.entropy_error = None;
                }
                Err(error) => {
                    self.entropy_error = Some(format!("解析结果失败：{error}"));
                }
            }
        }
    }

    fn render(&self, ui: &mut egui::Ui, summary: &PartitionSummary) {
        match summary {
            PartitionSummary::Harmony(harmony) => {
                badge_heading(
                    ui,
                    "HARMONY! 分区镜像",
                    egui::Color32::from_rgb(90, 170, 255),
                );
                self.render_harmony(ui, harmony);
            }
            PartitionSummary::Rvt(rvt) => {
                badge_heading(ui, "RVT 密钥镜像", egui::Color32::from_rgb(180, 130, 255));
                self.render_rvt(ui, rvt);
            }
            PartitionSummary::HvbWrapped {
                footer,
                cert,
                cert_error,
            } => {
                badge_heading(
                    ui,
                    "HVB 包装的分区镜像",
                    egui::Color32::from_rgb(90, 170, 255),
                );
                section(ui, "HVB 尾部");
                egui::Grid::new("hvb-footer-grid")
                    .num_columns(2)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        kv(ui, "证书偏移", crate::util::hex64(footer.cert_offset));
                        kv(ui, "证书大小", human_size(footer.cert_size));
                        kv(ui, "镜像大小", crate::util::hex64(footer.image_size));
                        kv(ui, "分区大小", crate::util::hex64(footer.partition_size));
                    });
                ui.add_space(6.0);
                section(ui, "HVB 证书");
                match cert {
                    Some(cert) => render_cert(ui, cert),
                    None => {
                        message_box(
                            ui,
                            egui::Color32::from_rgb(230, 170, 40),
                            format!(
                                "证书解析失败：{}",
                                cert_error.as_deref().unwrap_or("未知错误")
                            ),
                        );
                    }
                }
            }
        }
    }

    fn render_harmony(&self, ui: &mut egui::Ui, harmony: &HarmonySummary) {
        section(ui, "HARMONY! 头部");
        egui::Grid::new("harmony-hdr-grid")
            .num_columns(2)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                kv(ui, "头大小", crate::util::hex64(harmony.hdr_size as u64));
                kv(
                    ui,
                    "镜像大小",
                    crate::util::hex64(harmony.image_size as u64),
                );
                kv(ui, "标志", crate::util::hex64(harmony.flags as u64));
                kv(ui, "构建变体", &harmony.buildvariant);
            });
        ui.add_space(6.0);
        section(ui, "HVB 尾部");
        egui::Grid::new("harmony-footer-grid")
            .num_columns(2)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                kv(
                    ui,
                    "证书偏移",
                    crate::util::hex64(harmony.footer.cert_offset),
                );
                kv(ui, "证书大小", human_size(harmony.footer.cert_size));
                kv(
                    ui,
                    "镜像大小",
                    crate::util::hex64(harmony.footer.image_size),
                );
                kv(
                    ui,
                    "分区大小",
                    crate::util::hex64(harmony.footer.partition_size),
                );
            });
        ui.add_space(6.0);
        section(ui, "HVB 证书");
        render_cert(ui, &harmony.cert);
    }

    fn render_rvt(&self, ui: &mut egui::Ui, rvt: &common::formats::rvt::RvtInfo) {
        egui::Grid::new("rvt-overview-grid")
            .num_columns(2)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                kv(ui, "verity_num", rvt.verity_num.to_string());
                kv(
                    ui,
                    "每分区密钥数",
                    if rvt.raw_key_count == 0 {
                        "0(旧版, 按 1 处理)".to_owned()
                    } else {
                        rvt.raw_key_count.to_string()
                    },
                );
                kv(ui, "描述符数量", rvt.descriptors.len().to_string());
                kv(ui, "RVT 内容大小", format!("{} 字节", rvt.total_size));
                kv(
                    ui,
                    "包装方式",
                    if rvt.hvb_wrapped {
                        "HVB 包装".to_owned()
                    } else {
                        "裸 RVT".to_owned()
                    },
                );
                if let Some(footer) = &rvt.footer {
                    kv(ui, "分区大小", crate::util::hex64(footer.partition_size));
                }
            });
        if let Some(cert) = &rvt.cert {
            ui.add_space(6.0);
            section(ui, "HVB 证书");
            render_cert(ui, &cert_summary(cert));
        }
        if rvt.descriptors.is_empty() {
            return;
        }
        ui.add_space(6.0);
        section(ui, "分区公钥描述符");
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(130.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(140.0))
            .column(Column::remainder().at_least(140.0))
            .header(24.0, |mut header| {
                header.col(|ui| {
                    ui.strong("分区");
                });
                header.col(|ui| {
                    ui.strong("算法");
                });
                header.col(|ui| {
                    ui.strong("密钥长度");
                });
                header.col(|ui| {
                    ui.strong("公钥 SHA256(前 16 位)");
                });
                header.col(|ui| {
                    ui.strong("备份密钥");
                });
            })
            .body(|mut body| {
                for descriptor in &rvt.descriptors {
                    body.row(24.0, |mut row| {
                        row.col(|ui| {
                            ui.label(&descriptor.name);
                        });
                        row.col(|ui| {
                            ui.label(&descriptor.algorithm);
                        });
                        row.col(|ui| {
                            ui.label(format!("{} B", descriptor.pubkey_len));
                        });
                        row.col(|ui| {
                            let short = descriptor
                                .pubkey_sha256
                                .chars()
                                .take(16)
                                .collect::<String>();
                            ui.label(egui::RichText::new(format!("{short}…")).monospace().weak())
                                .on_hover_text(&descriptor.pubkey_sha256);
                        });
                        row.col(|ui| match &descriptor.backup_equals_main {
                            Some(true) => {
                                ui.label(egui::RichText::new("与主密钥相同").weak());
                            }
                            Some(false) => {
                                ui.label(
                                    egui::RichText::new("不同")
                                        .color(egui::Color32::from_rgb(230, 170, 40)),
                                );
                            }
                            None => {
                                ui.label(egui::RichText::new("无").weak());
                            }
                        });
                    });
                }
            });
    }
}

fn badge_heading(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("●").color(color).strong());
        ui.label(egui::RichText::new(text).strong().size(16.0));
    });
    ui.add_space(4.0);
}

fn render_cert(ui: &mut egui::Ui, cert: &CertSummary) {
    egui::Grid::new("cert-grid")
        .num_columns(2)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            kv(
                ui,
                "版本",
                format!("{}.{}", cert.version_major, cert.version_minor),
            );
            kv(ui, "分区名", &cert.partition_name);
            kv(
                ui,
                "原始镜像长度",
                crate::util::hex64(cert.image_original_len),
            );
            kv(ui, "镜像长度", crate::util::hex64(cert.image_len));
            kv(
                ui,
                "verity 类型",
                format!(
                    "{} ({})",
                    cert.verity_type,
                    match cert.verity_type {
                        1 => "hash",
                        2 => "hashtree",
                        _ => "?",
                    }
                ),
            );
            kv(
                ui,
                "哈希算法",
                format!(
                    "{} ({})",
                    cert.hash_algo,
                    match cert.hash_algo {
                        0 => "SHA256",
                        1 => "SHA128",
                        2 => "SHA512",
                        3 => "SM3",
                        _ => "?",
                    }
                ),
            );
            kv(ui, "盐长度", cert.salt_size.to_string());
            kv(ui, "摘要长度", cert.digest_size.to_string());
        });
}

fn cert_summary(cert: &common::formats::hvb::HvbCert) -> CertSummary {
    CertSummary {
        version_major: cert.version_major,
        version_minor: cert.version_minor,
        partition_name: cert.partition_name.clone(),
        image_original_len: cert.image_original_len,
        image_len: cert.image_len,
        verity_type: cert.verity_type,
        hash_algo: cert.hash_algo,
        salt_size: cert.salt_size,
        digest_size: cert.digest_size,
    }
}
