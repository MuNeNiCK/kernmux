use std::{future::Future, pin::Pin, rc::Rc};

use gpui::{
    App, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Root, Sizable, StyledExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    progress::Progress,
    scroll::ScrollableElement,
    sidebar::{Sidebar, SidebarHeader, SidebarMenu, SidebarMenuItem},
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use kernmux_api::v1::{
    Generation, ImageArtifact, Instance, InstanceState, Operation, OperationState, SnapshotHealth,
};
use kernmux_ui_model::{DataState, Intent, ManagementModel, ManagementSnapshot, Section};

pub type ManagementFuture =
    Pin<Box<dyn Future<Output = Result<ManagementSnapshot, String>> + 'static>>;

pub trait ManagementBackend {
    fn load_snapshot(&self) -> ManagementFuture;
    fn execute(&self, intent: Intent) -> ManagementFuture;
}

struct ManagementShell {
    model: ManagementModel,
    backend: Rc<dyn ManagementBackend>,
    action_error: Option<String>,
}

#[allow(clippy::redundant_closure_for_method_calls, clippy::unused_self)]
impl ManagementShell {
    fn new(
        backend: Rc<dyn ManagementBackend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut shell = Self {
            model: ManagementModel::loading(),
            backend,
            action_error: None,
        };
        shell.refresh(window, cx);
        shell
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let future = self.backend.load_snapshot();
        self.action_error = None;
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let _ = this.update_in(cx, |this, _, cx| {
                match result {
                    Ok(snapshot) => this.model.replace_snapshot(snapshot),
                    Err(message) => this.model.fail(message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn dispatch(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(message) = self.model.request(intent.clone()) {
            self.action_error = Some(message.to_owned());
            cx.notify();
            return;
        }
        self.action_error = None;
        let future = self.backend.execute(intent);
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let _ = this.update_in(cx, |this, _, cx| {
                match result {
                    Ok(snapshot) => this.model.replace_snapshot(snapshot),
                    Err(message) => {
                        this.model.reject_pending();
                        this.action_error = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Sidebar::new("host-navigation")
            .w(px(244.))
            .h_full()
            .border_0()
            .header(
                v_flex()
                    .w_full()
                    .gap_5()
                    .child(
                        SidebarHeader::new()
                            .w_full()
                            .child(
                                div()
                                    .size_9()
                                    .rounded(cx.theme().radius_lg)
                                    .bg(cx.theme().primary)
                                    .text_color(cx.theme().primary_foreground)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_semibold()
                                    .child("K"),
                            )
                            .child(
                                v_flex()
                                    .gap_0()
                                    .child(div().font_semibold().child("Kernmux"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Host management"),
                                    ),
                            ),
                    )
                    .child(self.host_identity(cx)),
            )
            .child(
                SidebarMenu::new().children(Section::ALL.into_iter().map(|section| {
                    SidebarMenuItem::new(section.label())
                        .icon(section_icon(section))
                        .active(self.model.section() == section)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.navigate(section);
                            cx.notify();
                        }))
                })),
            )
    }

    fn host_identity(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, detail, healthy) = match self.model.data() {
            DataState::Loading => ("Connecting", "Reading host inventory".to_owned(), false),
            DataState::Failed(_) => ("Unavailable", "Gateway connection failed".to_owned(), false),
            DataState::Ready(snapshot) => (
                if snapshot.host.health == SnapshotHealth::Healthy {
                    "Online"
                } else {
                    "Attention"
                },
                snapshot.host.kernel.release.clone(),
                snapshot.host.health == SnapshotHealth::Healthy,
            ),
        };
        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.18))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("CONTROL HOST"),
                    )
                    .child(if healthy {
                        Tag::success().small().outline().child(status)
                    } else {
                        Tag::warning().small().outline().child(status)
                    }),
            )
            .child(div().text_sm().font_medium().child(detail))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.model.section();
        let generation = match self.model.data() {
            DataState::Ready(snapshot) => Some(snapshot.host.generation.0),
            _ => None,
        };
        h_flex()
            .w_full()
            .min_h(px(76.))
            .px_6()
            .py_4()
            .gap_4()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xl().font_semibold().child(section.label()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(section_description(section)),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .children(generation.map(|value| {
                        Tag::secondary()
                            .outline()
                            .child(format!("Generation {value}"))
                    }))
                    .child(
                        Button::new("refresh-host")
                            .outline()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
    }

    fn content(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.model.data() {
            DataState::Loading => self.loading_state(cx).into_any_element(),
            DataState::Failed(message) => self.failed_state(message, cx).into_any_element(),
            DataState::Ready(snapshot) => match self.model.section() {
                Section::Overview => self.overview(snapshot, cx).into_any_element(),
                Section::Resources => self.resources(snapshot, cx).into_any_element(),
                Section::Instances => self.instances(snapshot, cx).into_any_element(),
                Section::Images => self.images(snapshot, cx).into_any_element(),
                Section::Operations => self.operations(snapshot, cx).into_any_element(),
            },
        };
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_6()
            .gap_4()
            .children(self.action_error.as_ref().map(|message| {
                Alert::error("action-error", message.clone()).title("Host action failed")
            }))
            .child(body)
            .when(window.bounds().size.width < px(1000.), |this| this.p_4())
    }

    fn loading_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .h_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(Spinner::new().large().color(cx.theme().primary))
            .child(div().font_medium().child("Connecting to control host"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading authoritative topology and instance state."),
            )
    }

    fn failed_state(&self, message: &str, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .max_w(px(680.))
            .mx_auto()
            .mt_12()
            .gap_4()
            .child(Alert::error("host-error", message.to_owned()).title("Control host unavailable"))
            .child(
                Button::new("retry-host")
                    .primary()
                    .label("Retry connection")
                    .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
            )
    }

    fn overview(&self, snapshot: &ManagementSnapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let host = &snapshot.host;
        let active = host
            .instances
            .iter()
            .filter(|item| item.state == InstanceState::Active)
            .count();
        v_flex()
            .w_full()
            .gap_5()
            .child(
                div()
                    .grid()
                    .grid_cols(4)
                    .gap_4()
                    .child(metric_card(
                        "Logical CPUs",
                        host.topology.cpus.len(),
                        "Online topology",
                        cx,
                    ))
                    .child(metric_card(
                        "Instances",
                        host.instances.len(),
                        &format!("{active} active"),
                        cx,
                    ))
                    .child(metric_card(
                        "NUMA nodes",
                        host.topology.numa_nodes.len(),
                        &host.topology.architecture,
                        cx,
                    ))
                    .child(metric_card(
                        "Verified images",
                        snapshot.images.len(),
                        "Content addressed",
                        cx,
                    )),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_4()
                    .child(memory_card(snapshot, cx))
                    .child(host_card(snapshot, cx)),
            )
            .children((host.health != SnapshotHealth::Healthy).then(|| {
                Alert::warning(
                    "host-health",
                    "Some host resources could not be verified. Mutations remain fail-closed.",
                )
                .title("Host state needs attention")
            }))
    }

    fn resources(&self, snapshot: &ManagementSnapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let host = &snapshot.host;
        v_flex()
            .w_full()
            .gap_5()
            .child(memory_card(snapshot, cx))
            .child(
                card(cx)
                    .child(card_heading(
                        "CPU topology",
                        "Hardware IDs delegated to the Multikernel pool",
                        cx,
                    ))
                    .child(div().flex().flex_wrap().gap_2().children(
                        host.topology.cpus.iter().map(|cpu| {
                            let delegated = host
                                .resource_pool
                                .cpu_hardware_ids
                                .contains(&cpu.hardware_id);
                            let available = host
                                .resource_pool
                                .available_cpu_hardware_ids
                                .contains(&cpu.hardware_id);
                            v_flex()
                                .w(px(82.))
                                .gap_1()
                                .p_2()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(if delegated {
                                    cx.theme().primary
                                } else {
                                    cx.theme().border
                                })
                                .bg(if delegated {
                                    cx.theme().primary.opacity(0.08)
                                } else {
                                    cx.theme().background
                                })
                                .child(div().font_medium().child(format!("CPU {}", cpu.logical_id)))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "Core {} · T{}",
                                            cpu.core_id, cpu.thread_index
                                        )),
                                )
                                .child(if available {
                                    Tag::success().small().outline().child("Free")
                                } else if delegated {
                                    Tag::info().small().outline().child("Assigned")
                                } else {
                                    Tag::secondary().small().outline().child("Host")
                                })
                        }),
                    )),
            )
            .child(
                card(cx)
                    .child(card_heading(
                        "NUMA placement",
                        "Memory locality exposed by the control host",
                        cx,
                    ))
                    .children(host.topology.numa_nodes.iter().map(|node| {
                        v_flex()
                            .gap_2()
                            .py_2()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(format!("Node {}", node.id))
                                    .child(format!(
                                        "{} available",
                                        bytes(node.available_memory_bytes)
                                    )),
                            )
                            .child(
                                Progress::new(("numa", node.id as usize))
                                    .value(percent(
                                        node.available_memory_bytes,
                                        node.total_memory_bytes,
                                    ))
                                    .accessibility_label(format!(
                                        "NUMA node {} available memory",
                                        node.id
                                    )),
                            )
                    })),
            )
    }

    fn instances(&self, snapshot: &ManagementSnapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let generation = snapshot.host.generation;
        let empty = snapshot.host.instances.is_empty();
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} managed peer kernels",
                                snapshot.host.instances.len()
                            )),
                    )
                    .child(
                        Button::new("new-instance")
                            .primary()
                            .icon(IconName::Plus)
                            .label("New instance")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.action_error = Some(
                                    "Open the host setup workflow to create an instance.".into(),
                                );
                                cx.notify();
                            })),
                    ),
            )
            .children(
                snapshot
                    .host
                    .instances
                    .iter()
                    .map(|instance| self.instance_row(instance, generation, cx)),
            )
            .children(empty.then(|| {
                empty_state(
                    "No kernel instances",
                    "Create an instance after delegating CPU and memory to the resource pool.",
                    cx,
                )
            }))
    }

    fn instance_row(
        &self,
        instance: &Instance,
        generation: Generation,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let action = match instance.state {
            InstanceState::Loaded => Some((
                "Start",
                IconName::Play,
                Intent::StartInstance {
                    id: instance.id,
                    expected_generation: generation,
                },
            )),
            InstanceState::Active => Some((
                "Stop",
                IconName::SquareTerminal,
                Intent::StopInstance {
                    id: instance.id,
                    expected_generation: generation,
                    force: false,
                },
            )),
            InstanceState::Ready => Some((
                "Delete",
                IconName::Delete,
                Intent::DeleteInstance {
                    id: instance.id,
                    expected_generation: generation,
                },
            )),
            InstanceState::Absent | InstanceState::Unknown => None,
        };
        card(cx)
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                h_flex()
                    .gap_4()
                    .min_w_0()
                    .child(
                        div()
                            .size_10()
                            .rounded(cx.theme().radius_lg)
                            .bg(cx.theme().secondary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Frame)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().font_semibold().child(instance.name.clone()))
                                    .child(instance_tag(instance.state)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "ID {} · {} CPUs · {} memory · {} devices",
                                        instance.id.0,
                                        instance.resources.cpu_hardware_ids.len(),
                                        bytes(instance.resources.memory_bytes),
                                        instance.resources.device_ids.len()
                                    )),
                            ),
                    ),
            )
            .children(action.map(|(label, icon, intent)| {
                Button::new(("instance-action", instance.id.0 as usize))
                    .outline()
                    .icon(icon)
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dispatch(intent.clone(), window, cx);
                    }))
            }))
    }

    fn images(&self, snapshot: &ManagementSnapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = snapshot.images.is_empty();
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Verified artifacts available to peer kernels"),
                    )
                    .child(
                        Button::new("import-image")
                            .primary()
                            .label("Import image")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.action_error = Some(
                                    "Open the host setup workflow to register an image.".into(),
                                );
                                cx.notify();
                            })),
                    ),
            )
            .children(snapshot.images.iter().map(|image| image_row(image, cx)))
            .children(empty.then(|| {
                empty_state(
                    "No verified images",
                    "Import a kernel or initrd into the content-addressed image store.",
                    cx,
                )
            }))
    }

    fn operations(
        &self,
        snapshot: &ManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = snapshot.host.operations.is_empty() && snapshot.host.transactions.is_empty();
        v_flex()
            .w_full()
            .gap_3()
            .children(
                snapshot
                    .host
                    .operations
                    .iter()
                    .map(|operation| operation_row(operation, cx)),
            )
            .children(snapshot.host.transactions.iter().map(|transaction| {
                card(cx)
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_medium()
                                    .child(format!("Transaction {}", transaction.id)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Atomic resource allocation change"),
                            ),
                    )
                    .child(
                        Tag::secondary()
                            .outline()
                            .child(format!("{:?}", transaction.state)),
                    )
            }))
            .children(empty.then(|| {
                empty_state(
                    "No recent operations",
                    "Lifecycle operations and resource transactions will appear here.",
                    cx,
                )
            }))
    }

    fn global_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(52.))
            .px_4()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.22))
            .child(
                h_flex()
                    .gap_3()
                    .child(div().font_semibold().child("Kernmux"))
                    .child(Tag::secondary().outline().child("Control Plane")),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Single host inventory"),
                    )
                    .child(
                        Button::new("global-refresh")
                            .outline()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
                    ),
            )
    }

    fn recent_tasks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let operations = match self.model.data() {
            DataState::Ready(snapshot) => snapshot.host.operations.as_slice(),
            _ => &[],
        };
        v_flex()
            .w_full()
            .h(px(126.))
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h(px(34.))
                    .px_4()
                    .justify_between()
                    .bg(cx.theme().secondary.opacity(0.18))
                    .child(div().text_sm().font_semibold().child("Recent Tasks"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} operations", operations.len())),
                    ),
            )
            .child(
                v_flex()
                    .px_4()
                    .py_2()
                    .gap_1()
                    .children(operations.iter().rev().take(3).map(|operation| {
                        h_flex()
                            .justify_between()
                            .text_sm()
                            .child(format!("{:?} · {}", operation.kind, operation.id.0))
                            .child(operation_tag(operation.state))
                    }))
                    .children(operations.is_empty().then(|| {
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No recent host tasks")
                    })),
            )
    }
}

impl Render for ManagementShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.global_header(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.navigation(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.header(cx))
                            .child(self.content(window, cx)),
                    ),
            )
            .child(self.recent_tasks(cx))
    }
}

fn card(cx: &App) -> gpui::Div {
    v_flex()
        .w_full()
        .gap_4()
        .p_5()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().secondary.opacity(0.18))
}

fn card_heading(title: &str, detail: &str, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().font_semibold().child(title.to_owned()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(detail.to_owned()),
        )
}

fn metric_card(label: &str, value: usize, detail: &str, cx: &App) -> impl IntoElement {
    card(cx)
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(div().text_2xl().font_semibold().child(value.to_string()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(detail.to_owned()),
        )
}

fn memory_card(snapshot: &ManagementSnapshot, cx: &App) -> impl IntoElement {
    let memory = &snapshot.host.memory;
    card(cx)
        .child(card_heading(
            "Memory allocation",
            "Assignable memory reserved for peer kernels",
            cx,
        ))
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_2xl()
                        .font_semibold()
                        .child(bytes(memory.assigned_bytes)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("of {} assigned", bytes(memory.assignable_bytes))),
                ),
        )
        .child(
            Progress::new("host-memory")
                .value(percent(memory.assigned_bytes, memory.assignable_bytes))
                .accessibility_label("Assigned Multikernel memory"),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "{} total · {} reserved by control kernel",
                    bytes(memory.total_bytes),
                    bytes(memory.host_reserved_bytes)
                )),
        )
}

fn host_card(snapshot: &ManagementSnapshot, cx: &App) -> impl IntoElement {
    let host = &snapshot.host;
    card(cx)
        .child(card_heading(
            "Control kernel",
            "Runtime identity and management capability",
            cx,
        ))
        .child(detail_line("Release", host.kernel.release.clone(), cx))
        .child(detail_line(
            "Architecture",
            host.topology.architecture.clone(),
            cx,
        ))
        .child(detail_line(
            "Capabilities",
            host.capabilities.len().to_string(),
            cx,
        ))
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Multikernel"),
                )
                .child(if host.kernel.multikernel_enabled {
                    Tag::success().outline().child("Enabled")
                } else {
                    Tag::danger().outline().child("Disabled")
                }),
        )
}

fn detail_line(label: &str, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .justify_between()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(div().text_sm().font_medium().child(value))
}

fn image_row(image: &ImageArtifact, cx: &App) -> impl IntoElement {
    card(cx)
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            h_flex()
                .gap_4()
                .child(
                    div()
                        .size_10()
                        .rounded(cx.theme().radius_lg)
                        .bg(cx.theme().secondary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::HardDrive)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(div().font_medium().child(image.id.clone()))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} · schema {}",
                                    bytes(image.bytes),
                                    image.schema_version
                                )),
                        ),
                ),
        )
        .child(Tag::info().outline().child(format!("{:?}", image.kind)))
}

fn operation_row(operation: &Operation, cx: &App) -> impl IntoElement {
    card(cx)
        .child(
            h_flex()
                .justify_between()
                .child(
                    v_flex()
                        .gap_1()
                        .child(div().font_medium().child(format!("{:?}", operation.kind)))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(operation.id.0.clone()),
                        ),
                )
                .child(operation_tag(operation.state)),
        )
        .children(operation.progress_percent.map(|value| {
            Progress::new(operation.id.0.clone())
                .value(f32::from(value))
                .accessibility_label("Operation progress")
        }))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Started {}", operation.created_at)),
        )
}

fn empty_state(title: &str, detail: &str, cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
        .items_center()
        .justify_center()
        .gap_2()
        .py_16()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            Icon::new(IconName::Inbox)
                .size_8()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().font_medium().child(title.to_owned()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(detail.to_owned()),
        )
}

fn instance_tag(state: InstanceState) -> Tag {
    match state {
        InstanceState::Active => Tag::success().outline().child("Active"),
        InstanceState::Loaded => Tag::info().outline().child("Loaded"),
        InstanceState::Ready => Tag::secondary().outline().child("Ready"),
        InstanceState::Absent => Tag::danger().outline().child("Absent"),
        InstanceState::Unknown => Tag::warning().outline().child("Unknown"),
    }
}

fn operation_tag(state: OperationState) -> Tag {
    match state {
        OperationState::Succeeded => Tag::success().outline().child("Succeeded"),
        OperationState::Running => Tag::info().outline().child("Running"),
        OperationState::Queued => Tag::secondary().outline().child("Queued"),
        OperationState::Failed | OperationState::Indeterminate => {
            Tag::danger().outline().child("Failed")
        }
        OperationState::Cancelled => Tag::warning().outline().child("Cancelled"),
        OperationState::Unknown => Tag::warning().outline().child("Unknown"),
    }
}

fn section_icon(section: Section) -> IconName {
    match section {
        Section::Overview => IconName::LayoutDashboard,
        Section::Resources => IconName::Cpu,
        Section::Instances => IconName::Frame,
        Section::Images => IconName::HardDrive,
        Section::Operations => IconName::ChartPie,
    }
}

fn section_description(section: Section) -> &'static str {
    match section {
        Section::Overview => "Host health, capacity, and active kernel domains",
        Section::Resources => "CPU topology, NUMA locality, memory, and device pools",
        Section::Instances => "Peer-kernel lifecycle and resource assignments",
        Section::Images => "Verified kernel and initrd artifacts",
        Section::Operations => "Asynchronous operations and resource transactions",
    }
}

fn percent(value: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        let whole = value.saturating_mul(100).saturating_div(total).min(100);
        f32::from(u8::try_from(whole).unwrap_or(100))
    }
}

fn bytes(value: u64) -> String {
    const GIB: u64 = 1_073_741_824;
    const MIB: u64 = 1_048_576;
    if value >= GIB {
        let tenths = value.saturating_mul(10).saturating_div(GIB);
        format!("{}.{:01} GiB", tenths / 10, tenths % 10)
    } else {
        format!("{} MiB", value.saturating_add(MIB / 2) / MIB)
    }
}

/// Opens the document-owned host management window.
///
/// # Panics
/// Panics if the browser platform cannot create the management window.
pub fn open_management_shell(cx: &mut App, backend: Rc<dyn ManagementBackend>) {
    gpui_component::init(cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.0), px(800.0)),
                cx,
            ))),
            window_min_size: Some(size(px(760.0), px(560.0))),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| ManagementShell::new(backend, window, cx));
            cx.new(|cx| Root::new(view, window, cx).bg(cx.theme().background))
        },
    )
    .expect("failed to open Kernmux management shell");
    cx.activate(true);
}
