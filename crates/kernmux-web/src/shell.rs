use std::{cell::OnceCell, rc::Rc};

use gpui::{
    App, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use kernmux_api::v1::{Generation, Instance, InstanceState};
use kernmux_ui_model::{DataState, Intent, ManagementModel, ManagementSnapshot, Section};

thread_local! {
    static SHELL: OnceCell<Entity<ManagementShell>> = const { OnceCell::new() };
    static INTENT_HANDLER: OnceCell<Rc<dyn Fn(Intent)>> = const { OnceCell::new() };
}

struct ManagementShell {
    model: ManagementModel,
}

impl ManagementShell {
    fn new() -> Self {
        Self {
            model: ManagementModel::loading(),
        }
    }

    fn nav_item(&self, section: Section, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let active = self.model.section() == section;
        div()
            .id(section.label())
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .font_weight(if active {
                gpui::FontWeight::SEMIBOLD
            } else {
                gpui::FontWeight::NORMAL
            })
            .text_color(if active {
                rgb(0x00e2_e8f0)
            } else {
                rgb(0x0094_a3b8)
            })
            .bg(if active {
                rgb(0x001e_293b)
            } else {
                rgb(0x000f_172a)
            })
            .hover(|style| style.bg(rgb(0x001e_293b)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.navigate(section);
                cx.notify();
            }))
            .child(section.label())
    }

    fn dispatch(&mut self, intent: Intent, cx: &mut Context<Self>) {
        if self.model.request(intent.clone()).is_err() {
            return;
        }
        INTENT_HANDLER.with(|slot| {
            if let Some(handler) = slot.get() {
                handler(intent);
            }
        });
        cx.notify();
    }

    fn action_button(
        &self,
        label: &'static str,
        key: (&'static str, u32),
        intent: Intent,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(key)
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .bg(rgb(0x001e_40af))
            .hover(|style| style.bg(rgb(0x001d_4ed8)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dispatch(intent.clone(), cx);
            }))
            .child(label)
    }

    fn instance_panel(
        &self,
        instance: &Instance,
        generation: Generation,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let action = match instance.state {
            InstanceState::Loaded => Some(self.action_button(
                "Start",
                ("start-instance", instance.id.0),
                Intent::StartInstance {
                    id: instance.id,
                    expected_generation: generation,
                },
                cx,
            )),
            InstanceState::Active => Some(self.action_button(
                "Stop",
                ("stop-instance", instance.id.0),
                Intent::StopInstance {
                    id: instance.id,
                    expected_generation: generation,
                    force: false,
                },
                cx,
            )),
            InstanceState::Ready => Some(self.action_button(
                "Delete",
                ("delete-instance", instance.id.0),
                Intent::DeleteInstance {
                    id: instance.id,
                    expected_generation: generation,
                },
                cx,
            )),
            InstanceState::Absent | InstanceState::Unknown => None,
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x001e_293b))
            .bg(rgb(0x000f_172a))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{} · {}", instance.name, instance.id.0)),
                    )
                    .child(div().text_sm().text_color(rgb(0x0094_a3b8)).child(format!(
                        "{:?} · {} CPUs · {} GiB",
                        instance.state,
                        instance.resources.cpu_hardware_ids.len(),
                        gib(instance.resources.memory_bytes)
                    ))),
            )
            .children(action)
    }

    #[allow(clippy::too_many_lines)]
    fn main_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match self.model.data() {
            DataState::Loading => {
                panel("Connecting to host", "Waiting for the management gateway.")
            }
            DataState::Failed(message) => panel("Host unavailable", message),
            DataState::Ready(snapshot) => match self.model.section() {
                Section::Overview => div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(section_title(
                        "Host overview",
                        "Authoritative Multikernel host state",
                    ))
                    .child(
                        div()
                            .grid()
                            .grid_cols(4)
                            .gap_3()
                            .child(metric(
                                "Logical CPUs",
                                snapshot.host.topology.cpus.len().to_string(),
                            ))
                            .child(metric(
                                "Instances",
                                snapshot.host.instances.len().to_string(),
                            ))
                            .child(metric("Images", snapshot.images.len().to_string()))
                            .child(metric("Generation", snapshot.host.generation.0.to_string())),
                    )
                    .child(panel(
                        "Control kernel",
                        format!(
                            "{} · Multikernel {}",
                            snapshot.host.kernel.release,
                            if snapshot.host.kernel.multikernel_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        ),
                    )),
                Section::Resources => div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(section_title(
                        "Resources",
                        "CPU, NUMA, memory, and device allocation",
                    ))
                    .child(panel(
                        "CPU pool",
                        format!(
                            "{} delegated · {} available",
                            snapshot.host.resource_pool.cpu_hardware_ids.len(),
                            snapshot.host.resource_pool.available_cpu_hardware_ids.len()
                        ),
                    ))
                    .child(panel(
                        "Memory",
                        format!(
                            "{} GiB assigned / {} GiB assignable",
                            gib(snapshot.host.memory.assigned_bytes),
                            gib(snapshot.host.memory.assignable_bytes)
                        ),
                    )),
                Section::Instances => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title(
                        "Instances",
                        "Peer-kernel lifecycle and assigned resources",
                    ))
                    .children(snapshot.host.instances.iter().map(|instance| {
                        self.instance_panel(instance, snapshot.host.generation, cx)
                    })),
                Section::Images => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title(
                        "Managed images",
                        "Verified immutable kernel and initrd artifacts",
                    ))
                    .children(snapshot.images.iter().map(|image| {
                        panel(
                            format!("{:?}", image.kind),
                            format!("{} · {} MiB", image.id, image.bytes / 1_048_576),
                        )
                    })),
                Section::Operations => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title(
                        "Operations",
                        "Asynchronous mutations and diagnostics",
                    ))
                    .children(snapshot.host.operations.iter().map(|operation| {
                        panel(
                            format!("{:?} · {}", operation.kind, operation.id.0),
                            format!(
                                "{:?} · generation {}",
                                operation.state, operation.expected_generation.0
                            ),
                        )
                    })),
            },
        }
    }
}

impl Render for ManagementShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0002_0617))
            .text_color(rgb(0x00e2_e8f0))
            .font_family("Inter")
            .child(
                div()
                    .h(px(58.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .border_b_1()
                    .border_color(rgb(0x001e_293b))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Kernmux"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x0064_748b))
                            .child("Host management"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(210.0))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .border_r_1()
                            .border_color(rgb(0x001e_293b))
                            .children(
                                Section::ALL
                                    .into_iter()
                                    .map(|section| self.nav_item(section, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("management-content")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .p_5()
                            .child(self.main_content(cx)),
                    ),
            )
    }
}

fn section_title(title: impl Into<String>, subtitle: impl Into<String>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .child(title.into()),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x0094_a3b8))
                .child(subtitle.into()),
        )
}

fn metric(label: impl Into<String>, value: impl Into<String>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(rgb(0x001e_293b))
        .bg(rgb(0x000f_172a))
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x0094_a3b8))
                .child(label.into()),
        )
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .child(value.into()),
        )
}

fn panel(title: impl Into<String>, detail: impl Into<String>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(rgb(0x001e_293b))
        .bg(rgb(0x000f_172a))
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x0094_a3b8))
                .child(detail.into()),
        )
}

fn gib(bytes: u64) -> u64 {
    bytes / 1_073_741_824
}

/// Opens the single-window host management shell.
///
/// # Panics
/// Panics when the browser platform cannot create its document-owned window,
/// or when the shell is initialized more than once in the same document.
pub fn open_management_shell(cx: &mut App) {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.0), px(800.0)),
                cx,
            ))),
            ..Default::default()
        },
        |_window, cx| {
            let shell = cx.new(|_| ManagementShell::new());
            SHELL.with(|slot| {
                assert!(
                    slot.set(shell.clone()).is_ok(),
                    "management shell already opened"
                );
            });
            shell
        },
    )
    .expect("failed to open Kernmux management shell");
    cx.activate(true);
}

pub fn install_management_snapshot(snapshot: ManagementSnapshot, cx: &mut App) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.get() {
            shell.update(cx, |shell, cx| {
                shell.model.replace_snapshot(snapshot);
                cx.notify();
            });
        }
    });
}

pub fn fail_management_shell(message: String, cx: &mut App) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.get() {
            shell.update(cx, |shell, cx| {
                shell.model.fail(message);
                cx.notify();
            });
        }
    });
}

/// Installs the browser transport adapter for renderer-neutral user intents.
///
/// # Panics
/// Panics when configured more than once in the same document.
pub fn set_intent_handler(handler: Rc<dyn Fn(Intent)>) {
    INTENT_HANDLER.with(|slot| {
        assert!(
            slot.set(handler).is_ok(),
            "intent handler already configured"
        );
    });
}
