use std::{future::Future, pin::Pin, rc::Rc};

use gpui::{
    App, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Root, Sizable, StyledExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    progress::Progress,
    scroll::ScrollableElement,
    sidebar::{Sidebar, SidebarHeader, SidebarMenu, SidebarMenuItem},
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use kernmux_api::v1::{
    Generation, ImageArtifact, ImageKind, Instance, InstanceId, InstanceState, Operation,
    OperationState, SnapshotHealth,
};
use kernmux_ui_model::{
    DataState, Intent, ManagementModel, ManagementSnapshot, Section, parse_cpu_hardware_ids,
    parse_memory_bytes,
};

pub type ManagementFuture =
    Pin<Box<dyn Future<Output = Result<ManagementSnapshot, String>> + 'static>>;

pub trait ManagementBackend {
    fn load_snapshot(&self) -> ManagementFuture;
    fn execute(&self, intent: Intent) -> ManagementFuture;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SetupPanel {
    ResourcePool,
    Instance,
    Image,
    LoadImage(InstanceId),
}

struct ManagementShell {
    model: ManagementModel,
    backend: Rc<dyn ManagementBackend>,
    action_error: Option<String>,
    setup: Option<SetupPanel>,
    pool_cpus: Entity<InputState>,
    pool_memory: Entity<InputState>,
    instance_id: Entity<InputState>,
    instance_name: Entity<InputState>,
    instance_cpus: Entity<InputState>,
    instance_memory: Entity<InputState>,
    image_kind: ImageKind,
    image_path: Entity<InputState>,
    image_expected_id: Entity<InputState>,
    load_kernel_id: Entity<InputState>,
    load_initrd_id: Entity<InputState>,
    load_command_line: Entity<InputState>,
}

#[allow(
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unused_self
)]
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
            setup: None,
            pool_cpus: cx
                .new(|cx| InputState::new(window, cx).placeholder("Hardware IDs, for example 4-7")),
            pool_memory: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("For example 8 GiB")
                    .default_value("8 GiB")
            }),
            instance_id: cx.new(|cx| InputState::new(window, cx).placeholder("Numeric ID")),
            instance_name: cx.new(|cx| InputState::new(window, cx).placeholder("build-node")),
            instance_cpus: cx
                .new(|cx| InputState::new(window, cx).placeholder("Pool CPU IDs, for example 4-5")),
            instance_memory: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("For example 2 GiB")
                    .default_value("2 GiB")
            }),
            image_kind: ImageKind::Kernel,
            image_path: cx.new(|cx| {
                InputState::new(window, cx).placeholder("/var/lib/kernmux/import/vmlinuz")
            }),
            image_expected_id: cx
                .new(|cx| InputState::new(window, cx).placeholder("Optional sha256: digest")),
            load_kernel_id: cx
                .new(|cx| InputState::new(window, cx).placeholder("sha256: kernel artifact ID")),
            load_initrd_id: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Optional sha256: initrd artifact ID")
            }),
            load_command_line: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Kernel command line")
                    .default_value("console=mktty0")
            }),
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
        let backend = Rc::clone(&self.backend);
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let refreshed = if result.is_err() {
                Some(backend.load_snapshot().await)
            } else {
                None
            };
            let _ = this.update_in(cx, |this, _, cx| {
                match result {
                    Ok(snapshot) => {
                        this.model.replace_snapshot(snapshot);
                        this.setup = None;
                    }
                    Err(message) => {
                        this.model.reject_pending();
                        if let Some(Ok(snapshot)) = refreshed {
                            this.model.replace_snapshot(snapshot);
                        }
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
            .children(self.setup.map(|setup| self.setup_panel(setup, cx)))
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

    fn setup_panel(&self, setup: SetupPanel, cx: &mut Context<Self>) -> gpui::Div {
        let (title, detail, fields, submit_label) = match setup {
            SetupPanel::ResourcePool => (
                "Configure resource pool",
                "Delegate CPUs and memory from the control kernel to managed peer kernels.",
                v_flex()
                    .gap_4()
                    .child(input_field(
                        "CPU hardware IDs",
                        "Comma-separated IDs and ranges from the host topology.",
                        Input::new(&self.pool_cpus),
                        cx,
                    ))
                    .child(input_field(
                        "Memory",
                        "Binary units are accepted: MiB, GiB, or TiB.",
                        Input::new(&self.pool_memory),
                        cx,
                    ))
                    .into_any_element(),
                "Apply pool",
            ),
            SetupPanel::Instance => (
                "Create kernel instance",
                "Reserve an identity and a subset of resources from the delegated pool.",
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_4()
                    .child(input_field(
                        "Instance ID",
                        "Stable numeric identifier.",
                        Input::new(&self.instance_id),
                        cx,
                    ))
                    .child(input_field(
                        "Name",
                        "Human-readable inventory name.",
                        Input::new(&self.instance_name),
                        cx,
                    ))
                    .child(input_field(
                        "CPU hardware IDs",
                        "Must be free in the resource pool.",
                        Input::new(&self.instance_cpus),
                        cx,
                    ))
                    .child(input_field(
                        "Memory",
                        "Must fit in unassigned pool memory.",
                        Input::new(&self.instance_memory),
                        cx,
                    ))
                    .into_any_element(),
                "Create instance",
            ),
            SetupPanel::Image => {
                let kernel = if self.image_kind == ImageKind::Kernel {
                    Button::new("kind-kernel").primary()
                } else {
                    Button::new("kind-kernel").outline()
                };
                let initrd = if self.image_kind == ImageKind::Initrd {
                    Button::new("kind-initrd").primary()
                } else {
                    Button::new("kind-initrd").outline()
                };
                (
                    "Register host artifact",
                    "Import a kernel or initrd from an administrator-controlled host path.",
                    v_flex()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().font_medium().child("Artifact type"))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(kernel.label("Kernel").on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.image_kind = ImageKind::Kernel;
                                                cx.notify();
                                            },
                                        )))
                                        .child(initrd.label("Initrd").on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.image_kind = ImageKind::Initrd;
                                                cx.notify();
                                            },
                                        ))),
                                ),
                        )
                        .child(input_field(
                            "Host path",
                            "The daemon only accepts paths beneath configured import roots.",
                            Input::new(&self.image_path),
                            cx,
                        ))
                        .child(input_field(
                            "Expected digest",
                            "Optional sha256 ID used to reject unexpected content.",
                            Input::new(&self.image_expected_id),
                            cx,
                        ))
                        .into_any_element(),
                    "Register artifact",
                )
            }
            SetupPanel::LoadImage(id) => (
                "Load instance image",
                "Attach verified artifacts to the ready instance before starting it.",
                v_flex()
                    .gap_4()
                    .child(input_field(
                        "Kernel artifact ID",
                        "Required content-addressed kernel image.",
                        Input::new(&self.load_kernel_id),
                        cx,
                    ))
                    .child(input_field(
                        "Initrd artifact ID",
                        "Optional content-addressed initrd image.",
                        Input::new(&self.load_initrd_id),
                        cx,
                    ))
                    .child(input_field(
                        "Command line",
                        "Optional boot arguments passed to the peer kernel.",
                        Input::new(&self.load_command_line),
                        cx,
                    ))
                    .child(
                        Tag::secondary()
                            .outline()
                            .child(format!("Target instance {}", id.0)),
                    )
                    .into_any_element(),
                "Load image",
            ),
        };

        card(cx)
            .border_color(cx.theme().primary)
            .child(
                h_flex()
                    .justify_between()
                    .child(card_heading(title, detail, cx))
                    .child(Button::new("close-setup").ghost().label("Close").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.setup = None;
                            this.action_error = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(fields)
            .child(
                h_flex().justify_end().child(
                    Button::new("submit-setup")
                        .primary()
                        .label(submit_label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.submit_setup(setup, window, cx);
                        })),
                ),
            )
    }

    fn submit_setup(&mut self, setup: SetupPanel, window: &mut Window, cx: &mut Context<Self>) {
        let generation = match self.model.data() {
            DataState::Ready(snapshot) => snapshot.host.generation,
            DataState::Loading | DataState::Failed(_) => {
                self.action_error = Some("Host data is not ready.".into());
                cx.notify();
                return;
            }
        };
        let intent = match self.setup_intent(setup, generation, cx) {
            Ok(intent) => intent,
            Err(message) => {
                self.action_error = Some(message);
                cx.notify();
                return;
            }
        };
        self.dispatch(intent, window, cx);
    }

    fn setup_intent(
        &self,
        setup: SetupPanel,
        generation: Generation,
        cx: &App,
    ) -> Result<Intent, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        match setup {
            SetupPanel::ResourcePool => Ok(Intent::ConfigurePool {
                expected_generation: generation,
                cpu_hardware_ids: parse_cpu_hardware_ids(&value(&self.pool_cpus))
                    .map_err(str::to_owned)?,
                memory_bytes: parse_memory_bytes(&value(&self.pool_memory))
                    .map_err(str::to_owned)?,
            }),
            SetupPanel::Instance => {
                let id = value(&self.instance_id)
                    .parse::<u32>()
                    .map_err(|_| "Instance ID must be a non-negative integer.".to_owned())?;
                let name = value(&self.instance_name);
                if name.is_empty() {
                    return Err("Instance name is required.".into());
                }
                Ok(Intent::CreateInstance {
                    expected_generation: generation,
                    id: InstanceId(id),
                    name,
                    cpu_hardware_ids: parse_cpu_hardware_ids(&value(&self.instance_cpus))
                        .map_err(str::to_owned)?,
                    memory_bytes: parse_memory_bytes(&value(&self.instance_memory))
                        .map_err(str::to_owned)?,
                })
            }
            SetupPanel::Image => {
                let source_path = value(&self.image_path);
                if source_path.is_empty() {
                    return Err("Host path is required.".into());
                }
                let expected_id = value(&self.image_expected_id);
                Ok(Intent::ImportImage {
                    expected_generation: generation,
                    kind: self.image_kind,
                    source_path,
                    expected_id: (!expected_id.is_empty()).then_some(expected_id),
                })
            }
            SetupPanel::LoadImage(id) => {
                let kernel_id = value(&self.load_kernel_id);
                if kernel_id.is_empty() {
                    return Err("Kernel artifact ID is required.".into());
                }
                let initrd_id = value(&self.load_initrd_id);
                let command_line = value(&self.load_command_line);
                Ok(Intent::LoadInstanceImage {
                    id,
                    expected_generation: generation,
                    kernel_id,
                    initrd_id: (!initrd_id.is_empty()).then_some(initrd_id),
                    command_line: (!command_line.is_empty()).then_some(command_line),
                })
            }
        }
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
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} CPUs delegated · {} pool memory",
                                host.resource_pool.cpu_hardware_ids.len(),
                                bytes(
                                    host.resource_pool
                                        .memory_regions
                                        .iter()
                                        .map(|region| region.bytes)
                                        .sum()
                                )
                            )),
                    )
                    .child(
                        Button::new("configure-pool")
                            .primary()
                            .label("Configure pool")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.setup = Some(SetupPanel::ResourcePool);
                                this.action_error = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(memory_card(snapshot, cx))
            .child(
                card(cx)
                    .child(card_heading(
                        "CPU topology",
                        "Hardware IDs delegated to the Multikernel pool",
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(host.topology.cpus.iter().map(|cpu| {
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
                                    .child(
                                        div()
                                            .font_medium()
                                            .child(format!("CPU {}", cpu.logical_id)),
                                    )
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
                            }))
                            .children(
                                host.resource_pool
                                    .cpu_hardware_ids
                                    .iter()
                                    .filter(|hardware_id| {
                                        !host
                                            .topology
                                            .cpus
                                            .iter()
                                            .any(|cpu| cpu.hardware_id == **hardware_id)
                                    })
                                    .map(|hardware_id| {
                                        let available = host
                                            .resource_pool
                                            .available_cpu_hardware_ids
                                            .contains(hardware_id);
                                        v_flex()
                                            .w(px(82.))
                                            .gap_1()
                                            .p_2()
                                            .rounded(cx.theme().radius)
                                            .border_1()
                                            .border_color(cx.theme().primary)
                                            .bg(cx.theme().primary.opacity(0.08))
                                            .child(
                                                div()
                                                    .font_medium()
                                                    .child(format!("APIC {hardware_id}")),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Delegated ID"),
                                            )
                                            .child(if available {
                                                Tag::success().small().outline().child("Free")
                                            } else {
                                                Tag::info().small().outline().child("Assigned")
                                            })
                                    }),
                            ),
                    ),
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
                                this.setup = Some(SetupPanel::Instance);
                                this.action_error = None;
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
            .child(
                h_flex()
                    .gap_2()
                    .children((instance.state == InstanceState::Ready).then(|| {
                        let id = instance.id;
                        Button::new(("load-instance", id.0 as usize))
                            .primary()
                            .label("Load image")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.setup = Some(SetupPanel::LoadImage(id));
                                this.action_error = None;
                                cx.notify();
                            }))
                    }))
                    .children((instance.state == InstanceState::Loaded).then(|| {
                        let intent = Intent::UnloadInstance {
                            id: instance.id,
                            expected_generation: generation,
                        };
                        Button::new(("unload-instance", instance.id.0 as usize))
                            .outline()
                            .label("Unload")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dispatch(intent.clone(), window, cx);
                            }))
                    }))
                    .children(action.map(|(label, icon, intent)| {
                        Button::new(("instance-action", instance.id.0 as usize))
                            .outline()
                            .icon(icon)
                            .label(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dispatch(intent.clone(), window, cx);
                            }))
                    })),
            )
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
                            .label("Register host artifact")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.setup = Some(SetupPanel::Image);
                                this.action_error = None;
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

fn input_field(label: &str, detail: &str, input: Input, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(div().text_sm().font_medium().child(label.to_owned()))
        .child(input.aria_label(label.to_owned()))
        .child(
            div()
                .text_xs()
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
