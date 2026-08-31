use std::{future::Future, pin::Pin, rc::Rc};

use gpui::{
    App, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Selectable, Sizable, StyledExt,
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
    OperationKind, OperationState, ResourceKind, SnapshotHealth, Transaction, TransactionState,
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
    DeleteInstance(InstanceId),
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
    recent_tasks_collapsed: bool,
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
            recent_tasks_collapsed: false,
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
        let instances = match self.model.data() {
            DataState::Ready(snapshot) => snapshot
                .host
                .instances
                .iter()
                .map(|instance| (instance.id, instance.name.clone()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let selected = self.model.selected_instance();
        let instance_count = instances.len();
        let instance_items = instances
            .into_iter()
            .map(|(id, name)| {
                let monitor = SidebarMenuItem::new("Monitor")
                    .icon(IconName::ChartPie)
                    .active(this_section_is_instance_monitor(
                        self.model.section(),
                        selected,
                        id,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.action_error = None;
                        this.model.select_instance(Some(id));
                        this.model.navigate(Section::Operations);
                        cx.notify();
                    }));
                SidebarMenuItem::new(name)
                    .icon(IconName::Frame)
                    .active(selected == Some(id) && self.model.section() == Section::Instances)
                    .default_open(selected == Some(id))
                    .click_to_open(true)
                    .children([monitor])
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.action_error = None;
                        this.model.navigate(Section::Instances);
                        this.model.select_instance(Some(id));
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();

        Sidebar::new("host-navigation")
            .w(px(196.))
            .h_full()
            .border_0()
            .header(
                SidebarHeader::new()
                    .w_full()
                    .child(Icon::new(IconName::LayoutDashboard).size_5())
                    .child(
                        v_flex()
                            .gap_0()
                            .child(div().text_sm().font_semibold().child("Navigator"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Single host inventory"),
                            ),
                    ),
            )
            .child(
                SidebarMenu::new()
                    .child(
                        SidebarMenuItem::new("Host")
                            .icon(IconName::LayoutDashboard)
                            .active(self.model.section() == Section::Overview)
                            .default_open(true)
                            .children([
                                SidebarMenuItem::new("Manage")
                                    .icon(IconName::Cpu)
                                    .active(self.model.section() == Section::Resources)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.action_error = None;
                                        this.model.select_instance(None);
                                        this.model.navigate(Section::Resources);
                                        cx.notify();
                                    })),
                                SidebarMenuItem::new("Monitor")
                                    .icon(IconName::ChartPie)
                                    .active(
                                        self.model.section() == Section::Operations
                                            && self.model.selected_instance().is_none(),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.action_error = None;
                                        this.model.select_instance(None);
                                        this.model.navigate(Section::Operations);
                                        cx.notify();
                                    })),
                            ])
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.action_error = None;
                                this.model.select_instance(None);
                                this.model.navigate(Section::Overview);
                                cx.notify();
                            })),
                    )
                    .child(
                        SidebarMenuItem::new(format!("Instances  {instance_count}"))
                            .icon(IconName::Frame)
                            .active(
                                self.model.section() == Section::Instances
                                    && self.model.selected_instance().is_none(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.action_error = None;
                                this.model.select_instance(None);
                                this.model.navigate(Section::Instances);
                                cx.notify();
                            }))
                            .default_open(true)
                            .children(instance_items),
                    )
                    .child(
                        SidebarMenuItem::new("Images")
                            .icon(IconName::HardDrive)
                            .active(self.model.section() == Section::Images)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.action_error = None;
                                this.model.select_instance(None);
                                this.model.navigate(Section::Images);
                                cx.notify();
                            })),
                    ),
            )
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.model.section();
        let selected_instance = match (self.model.data(), self.model.selected_instance()) {
            (DataState::Ready(snapshot), Some(id)) => snapshot
                .host
                .instances
                .iter()
                .find(|instance| instance.id == id),
            _ => None,
        };
        let (title, detail) = selected_instance.map_or_else(
            || {
                (
                    section.label().to_owned(),
                    section_description(section).to_owned(),
                )
            },
            |instance| {
                (
                    instance.name.clone(),
                    format!(
                        "Instance {} · {}",
                        instance.id.0,
                        instance_state_label(instance.state)
                    ),
                )
            },
        );
        let object_tabs = if let Some(instance) = selected_instance {
            let id = instance.id;
            h_flex()
                .h(px(36.))
                .px_3()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    Button::new(("instance-summary-tab", id.0 as usize))
                        .ghost()
                        .small()
                        .label("Summary")
                        .selected(section == Section::Instances)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.action_error = None;
                            this.model.select_instance(Some(id));
                            this.model.navigate(Section::Instances);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(("instance-monitor-tab", id.0 as usize))
                        .ghost()
                        .small()
                        .label("Monitor")
                        .selected(section == Section::Operations)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.action_error = None;
                            this.model.select_instance(Some(id));
                            this.model.navigate(Section::Operations);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(("instance-manage-tab", id.0 as usize))
                        .ghost()
                        .small()
                        .label("Manage")
                        .selected(section == Section::Resources)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.action_error = None;
                            this.model.select_instance(Some(id));
                            this.model.navigate(Section::Resources);
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else if matches!(
            section,
            Section::Overview | Section::Resources | Section::Operations
        ) {
            h_flex()
                .h(px(36.))
                .px_3()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    Button::new("host-summary-tab")
                        .ghost()
                        .small()
                        .label("Summary")
                        .selected(section == Section::Overview)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.action_error = None;
                            this.model.navigate(Section::Overview);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("host-monitor-tab")
                        .ghost()
                        .small()
                        .label("Monitor")
                        .selected(section == Section::Operations)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.action_error = None;
                            this.model.navigate(Section::Operations);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("host-manage-tab")
                        .ghost()
                        .small()
                        .label("Manage")
                        .selected(section == Section::Resources)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.action_error = None;
                            this.model.navigate(Section::Resources);
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            div().h(px(1.)).into_any_element()
        };

        v_flex()
            .w_full()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .min_h(px(58.))
                    .px_4()
                    .py_2()
                    .gap_3()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_3()
                            .child(
                                div()
                                    .size_9()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(Icon::new(if selected_instance.is_some() {
                                        IconName::Frame
                                    } else {
                                        IconName::LayoutDashboard
                                    })),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap_0()
                                    .child(div().text_lg().font_semibold().child(title))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(detail),
                                    ),
                            ),
                    )
                    .children(selected_instance.map(|instance| instance_tag(instance.state))),
            )
            .child(object_tabs)
    }

    fn content(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.model.data() {
            DataState::Loading => self.loading_state(cx).into_any_element(),
            DataState::Failed(message) => self.failed_state(message, cx).into_any_element(),
            DataState::Ready(snapshot) => match self.model.section() {
                Section::Overview => self.overview(snapshot, cx).into_any_element(),
                Section::Resources => {
                    if let Some(id) = self.model.selected_instance() {
                        self.instance_manage(snapshot, id, cx).into_any_element()
                    } else {
                        self.resources(snapshot, cx).into_any_element()
                    }
                }
                Section::Instances => self.instances(snapshot, cx).into_any_element(),
                Section::Images => self.images(snapshot, cx).into_any_element(),
                Section::Operations => {
                    if let Some(id) = self.model.selected_instance() {
                        self.instance_monitor(snapshot, id, cx).into_any_element()
                    } else {
                        self.operations(snapshot, cx).into_any_element()
                    }
                }
            },
        };
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_3()
            .gap_3()
            .children(self.action_error.as_ref().map(|message| {
                Alert::error("action-error", message.clone()).title("Host action failed")
            }))
            .children(self.setup.map(|setup| self.setup_panel(setup, cx)))
            .child(body)
            .when(window.bounds().size.width < px(1000.), |this| this.p_2())
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
            SetupPanel::DeleteInstance(id) => (
                "Delete instance",
                "Permanently remove this instance from the host inventory.",
                v_flex()
                    .gap_3()
                    .child(
                        Alert::warning(
                            "confirm-delete-instance",
                            "The instance must be ready. This operation cannot be undone.",
                        )
                        .title(format!("Delete instance {}?", id.0)),
                    )
                    .child(
                        Tag::secondary()
                            .outline()
                            .child(format!("Resolved target: instance {}", id.0)),
                    )
                    .into_any_element(),
                "Delete instance",
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
            SetupPanel::DeleteInstance(id) => Ok(Intent::DeleteInstance {
                id,
                expected_generation: generation,
            }),
        }
    }

    fn overview(&self, snapshot: &ManagementSnapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let host = &snapshot.host;
        let active = host
            .instances
            .iter()
            .filter(|item| item.state == InstanceState::Active)
            .count();
        let logical_cpus = host.topology.cpus.len()
            + host
                .resource_pool
                .cpu_hardware_ids
                .iter()
                .filter(|id| !host.topology.cpus.iter().any(|cpu| cpu.hardware_id == **id))
                .count();
        let delegated_cpus = host.resource_pool.cpu_hardware_ids.len();

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .min_h(px(38.))
                    .gap_2()
                    .child(
                        Button::new("configure-host")
                            .primary()
                            .small()
                            .label("Configure resource pool")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.setup = Some(SetupPanel::ResourcePool);
                                this.action_error = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("refresh-host-summary")
                            .outline()
                            .small()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .children((host.health != SnapshotHealth::Healthy).then(|| {
                Alert::warning(
                    "host-health",
                    "Some host resources could not be verified. Mutations remain fail-closed.",
                )
                .title("Host state needs attention")
            }))
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_3()
                    .child(
                        property_panel("Host details", cx)
                            .child(property_row("Kernel", host.kernel.release.clone(), cx))
                            .child(property_row(
                                "Architecture",
                                host.topology.architecture.clone(),
                                cx,
                            ))
                            .child(property_row(
                                "Multikernel",
                                if host.kernel.multikernel_enabled {
                                    "Enabled"
                                } else {
                                    "Disabled"
                                }
                                .to_owned(),
                                cx,
                            ))
                            .child(property_row("Health", format!("{:?}", host.health), cx)),
                    )
                    .child(
                        property_panel("Capacity", cx)
                            .child(utilization_row(
                                "CPU delegated",
                                format!("{delegated_cpus} of {logical_cpus}"),
                                percent_usize(delegated_cpus, logical_cpus),
                                "CPUs delegated to peer kernels",
                                cx,
                            ))
                            .child(utilization_row(
                                "Memory assigned",
                                format!(
                                    "{} of {}",
                                    bytes(host.memory.assigned_bytes),
                                    bytes(host.memory.assignable_bytes)
                                ),
                                percent(host.memory.assigned_bytes, host.memory.assignable_bytes),
                                "Memory assigned to peer kernels",
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_3()
                    .child(
                        property_panel("Inventory", cx)
                            .child(property_row(
                                "Kernel instances",
                                host.instances.len().to_string(),
                                cx,
                            ))
                            .child(property_row("Active instances", active.to_string(), cx))
                            .child(property_row(
                                "Registered images",
                                snapshot.images.len().to_string(),
                                cx,
                            ))
                            .child(property_row(
                                "Recent tasks",
                                host.operations.len().to_string(),
                                cx,
                            )),
                    )
                    .child(
                        property_panel("Hardware", cx)
                            .child(property_row("Logical CPUs", logical_cpus.to_string(), cx))
                            .child(property_row(
                                "NUMA nodes",
                                host.topology.numa_nodes.len().to_string(),
                                cx,
                            ))
                            .child(property_row(
                                "Physical memory",
                                bytes(host.memory.total_bytes),
                                cx,
                            ))
                            .child(property_row(
                                "Control-kernel reservation",
                                bytes(host.memory.host_reserved_bytes),
                                cx,
                            )),
                    ),
            )
    }

    #[allow(dead_code)]
    fn overview_legacy(
        &self,
        snapshot: &ManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let host = &snapshot.host;
        let active = host
            .instances
            .iter()
            .filter(|item| item.state == InstanceState::Active)
            .count();
        let logical_cpus = host.topology.cpus.len()
            + host
                .resource_pool
                .cpu_hardware_ids
                .iter()
                .filter(|id| !host.topology.cpus.iter().any(|cpu| cpu.hardware_id == **id))
                .count();
        let delegated_cpus = host.resource_pool.cpu_hardware_ids.len();
        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("configure-host")
                            .primary()
                            .label("Configure resource pool")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.setup = Some(SetupPanel::ResourcePool);
                                this.action_error = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("refresh-host-summary")
                            .outline()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_4()
                    .child(
                        summary_panel("Host", cx)
                            .child(detail_line("Kernel", host.kernel.release.clone(), cx))
                            .child(detail_line(
                                "Architecture",
                                host.topology.architecture.clone(),
                                cx,
                            ))
                            .child(detail_line(
                                "Instances",
                                format!("{} registered · {active} active", host.instances.len()),
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
                            ),
                    )
                    .child(
                        summary_panel("Resource allocation", cx)
                            .child(detail_line(
                                "CPU",
                                format!("{delegated_cpus} delegated of {logical_cpus}"),
                                cx,
                            ))
                            .child(
                                Progress::new("host-cpu-allocation")
                                    .value(percent_usize(delegated_cpus, logical_cpus))
                                    .accessibility_label("CPUs delegated to peer kernels"),
                            )
                            .child(detail_line(
                                "Memory",
                                format!(
                                    "{} assigned of {}",
                                    bytes(host.memory.assigned_bytes),
                                    bytes(host.memory.assignable_bytes)
                                ),
                                cx,
                            ))
                            .child(
                                Progress::new("host-memory-allocation")
                                    .value(percent(
                                        host.memory.assigned_bytes,
                                        host.memory.assignable_bytes,
                                    ))
                                    .accessibility_label("Memory assigned to peer kernels"),
                            ),
                    ),
            )
            .children((host.health != SnapshotHealth::Healthy).then(|| {
                Alert::warning(
                    "host-health",
                    "Some host resources could not be verified. Mutations remain fail-closed.",
                )
                .title("Host state needs attention")
            }))
            .child(
                summary_section("Hardware", cx)
                    .child(detail_line("Logical CPUs", logical_cpus.to_string(), cx))
                    .child(detail_line(
                        "NUMA nodes",
                        host.topology.numa_nodes.len().to_string(),
                        cx,
                    ))
                    .child(detail_line(
                        "Physical memory",
                        bytes(host.memory.total_bytes),
                        cx,
                    ))
                    .child(detail_line(
                        "Control-kernel reservation",
                        bytes(host.memory.host_reserved_bytes),
                        cx,
                    )),
            )
            .child(
                summary_section("Configuration", cx)
                    .child(detail_line(
                        "Verified images",
                        snapshot.images.len().to_string(),
                        cx,
                    ))
                    .child(detail_line(
                        "Capabilities",
                        host.capabilities.len().to_string(),
                        cx,
                    )),
            )
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

    fn instances(&self, snapshot: &ManagementSnapshot, cx: &mut Context<Self>) -> gpui::Div {
        if let Some(instance) = self.model.selected_instance().and_then(|id| {
            snapshot
                .host
                .instances
                .iter()
                .find(|instance| instance.id == id)
        }) {
            return self.instance_summary(snapshot, instance, cx);
        }

        let generation = snapshot.host.generation;
        let empty = snapshot.host.instances.is_empty();
        v_flex()
            .w_full()
            .gap_0()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_h(px(52.))
                    .px_3()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("new-instance")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label("Create / Register instance")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.setup = Some(SetupPanel::Instance);
                                        this.action_error = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("refresh-instances")
                                    .outline()
                                    .label("Refresh")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.refresh(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} instances", snapshot.host.instances.len())),
                    ),
            )
            .child(instance_table_header(cx))
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
        _generation: Generation,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = instance.id;
        h_flex()
            .id(("instance-row", id.0 as usize))
            .w_full()
            .min_h(px(46.))
            .items_center()
            .px_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().secondary.opacity(0.35)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.action_error = None;
                this.model.navigate(Section::Instances);
                this.model.select_instance(Some(id));
                cx.notify();
            }))
            .child(
                h_flex()
                    .w(px(260.))
                    .gap_2()
                    .min_w_0()
                    .child(Icon::new(IconName::Frame))
                    .child(div().font_medium().child(instance.name.clone())),
            )
            .child(div().w(px(140.)).pr_4().child(instance_tag(instance.state)))
            .child(
                div()
                    .w(px(90.))
                    .text_sm()
                    .child(instance.resources.cpu_hardware_ids.len().to_string()),
            )
            .child(
                div()
                    .w(px(130.))
                    .text_sm()
                    .child(bytes(instance.resources.memory_bytes)),
            )
            .child(div().flex_1().text_sm().child(if instance.image.present {
                "Loaded"
            } else {
                "Not loaded"
            }))
            .child(
                div()
                    .w(px(72.))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(instance.id.0.to_string()),
            )
    }

    fn instance_summary(
        &self,
        snapshot: &ManagementSnapshot,
        instance: &Instance,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let generation = snapshot.host.generation;
        let id = instance.id;
        let lifecycle = match instance.state {
            InstanceState::Loaded => Some((
                "Start",
                Intent::StartInstance {
                    id,
                    expected_generation: generation,
                },
            )),
            InstanceState::Active => Some((
                "Stop",
                Intent::StopInstance {
                    id,
                    expected_generation: generation,
                    force: false,
                },
            )),
            _ => None,
        };
        let assigned_memory = instance.resources.memory_bytes;
        let pool_memory = snapshot
            .host
            .resource_pool
            .memory_regions
            .iter()
            .map(|region| region.bytes)
            .sum::<u64>();

        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new(("console", id.0 as usize))
                            .outline()
                            .icon(IconName::SquareTerminal)
                            .label("Console")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dispatch(Intent::OpenConsole(id), window, cx);
                            })),
                    )
                    .child(
                        Button::new(("monitor", id.0 as usize))
                            .outline()
                            .label("Monitor")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.action_error = None;
                                this.model.select_instance(Some(id));
                                this.model.navigate(Section::Operations);
                                cx.notify();
                            })),
                    )
                    .children(lifecycle.map(|(label, intent)| {
                        Button::new(("lifecycle", id.0 as usize))
                            .primary()
                            .label(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dispatch(intent.clone(), window, cx);
                            }))
                    }))
                    .children((instance.state == InstanceState::Ready).then(|| {
                        Button::new(("load-image", id.0 as usize))
                            .primary()
                            .label("Load image")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.setup = Some(SetupPanel::LoadImage(id));
                                this.action_error = None;
                                cx.notify();
                            }))
                    }))
                    .children((instance.state == InstanceState::Ready).then(|| {
                        Button::new(("instance-actions", id.0 as usize))
                            .outline()
                            .label("Actions")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.setup = Some(SetupPanel::DeleteInstance(id));
                                this.action_error = None;
                                cx.notify();
                            }))
                    }))
                    .children((instance.state == InstanceState::Loaded).then(|| {
                        let intent = Intent::UnloadInstance {
                            id,
                            expected_generation: generation,
                        };
                        Button::new(("unload", id.0 as usize))
                            .outline()
                            .label("Unload")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dispatch(intent.clone(), window, cx);
                            }))
                    }))
                    .child(
                        Button::new(("refresh-instance", id.0 as usize))
                            .outline()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap_4()
                    .child(
                        v_flex()
                            .min_h(px(190.))
                            .border_1()
                            .border_color(cx.theme().border)
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .child(Icon::new(IconName::SquareTerminal).size_8())
                            .child(div().font_medium().child("MKTTY console"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if instance.state == InstanceState::Active {
                                        "Open the console to attach"
                                    } else {
                                        "Console is available while active"
                                    }),
                            ),
                    )
                    .child(
                        summary_panel("Instance", cx)
                            .child(detail_line("Name", instance.name.clone(), cx))
                            .child(detail_line(
                                "State",
                                instance_state_label(instance.state),
                                cx,
                            ))
                            .child(detail_line("ID", instance.id.0.to_string(), cx))
                            .child(detail_line(
                                "Kernel image",
                                if instance.image.present {
                                    "Loaded"
                                } else {
                                    "Not loaded"
                                }
                                .to_owned(),
                                cx,
                            )),
                    )
                    .child(
                        summary_panel("Assigned resources", cx)
                            .child(detail_line(
                                "CPU",
                                format!(
                                    "{} logical CPUs",
                                    instance.resources.cpu_hardware_ids.len()
                                ),
                                cx,
                            ))
                            .child(detail_line("Memory", bytes(assigned_memory), cx))
                            .child(
                                Progress::new(("instance-memory", id.0 as usize))
                                    .value(percent(assigned_memory, pool_memory))
                                    .accessibility_label("Instance share of resource-pool memory"),
                            )
                            .child(detail_line(
                                "Devices",
                                instance.resources.device_ids.len().to_string(),
                                cx,
                            )),
                    ),
            )
            .child(
                summary_section("General Information", cx)
                    .child(detail_line(
                        "Lifecycle state",
                        instance_state_label(instance.state),
                        cx,
                    ))
                    .child(detail_line(
                        "Image state",
                        if instance.image.present {
                            "Present"
                        } else {
                            "Not loaded"
                        }
                        .to_owned(),
                        cx,
                    )),
            )
            .child(
                summary_section("Performance summary — last hour", cx).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Historical CPU and memory telemetry is not available from this host.",
                        ),
                ),
            )
            .child(
                summary_section("Hardware Configuration", cx)
                    .child(detail_line(
                        "CPU hardware IDs",
                        hardware_ids(&instance.resources.cpu_hardware_ids),
                        cx,
                    ))
                    .child(detail_line(
                        "Memory",
                        bytes(instance.resources.memory_bytes),
                        cx,
                    ))
                    .child(detail_line(
                        "Device assignments",
                        if instance.resources.device_ids.is_empty() {
                            "None".to_owned()
                        } else {
                            instance.resources.device_ids.join(", ")
                        },
                        cx,
                    )),
            )
            .child(
                summary_section("Resource Consumption", cx)
                    .child(detail_line(
                        "Assigned host memory",
                        bytes(instance.resources.memory_bytes),
                        cx,
                    ))
                    .child(detail_line(
                        "Observed usage",
                        "Runtime telemetry unavailable".to_owned(),
                        cx,
                    )),
            )
    }

    fn instance_manage(
        &self,
        snapshot: &ManagementSnapshot,
        id: InstanceId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let Some(instance) = snapshot.host.instances.iter().find(|item| item.id == id) else {
            return v_flex().w_full().child(Alert::error(
                "missing-instance",
                "The selected instance no longer exists.",
            ));
        };
        let generation = snapshot.host.generation;
        let unload = Intent::UnloadInstance {
            id,
            expected_generation: generation,
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .min_h(px(38.))
                    .gap_2()
                    .child(
                        Button::new(("manage-load-image", id.0 as usize))
                            .primary()
                            .label(if instance.image.present {
                                "Replace kernel image"
                            } else {
                                "Load kernel image"
                            })
                            .disabled(instance.state != InstanceState::Ready)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.setup = Some(SetupPanel::LoadImage(id));
                                this.action_error = None;
                                cx.notify();
                            })),
                    )
                    .children((instance.state == InstanceState::Loaded).then(|| {
                        Button::new(("manage-unload", id.0 as usize))
                            .outline()
                            .label("Unload image")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dispatch(unload.clone(), window, cx);
                            }))
                    }))
                    .child(
                        Button::new(("manage-refresh", id.0 as usize))
                            .outline()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_3()
                    .child(
                        property_panel("Resource assignment", cx)
                            .child(property_row(
                                "CPU hardware IDs",
                                hardware_ids(&instance.resources.cpu_hardware_ids),
                                cx,
                            ))
                            .child(property_row(
                                "Logical CPU count",
                                instance.resources.cpu_hardware_ids.len().to_string(),
                                cx,
                            ))
                            .child(property_row(
                                "Assigned memory",
                                bytes(instance.resources.memory_bytes),
                                cx,
                            ))
                            .child(property_row(
                                "Memory base",
                                instance.resources.memory_base.map_or_else(
                                    || "Not reported".to_owned(),
                                    |base| format!("0x{base:x}"),
                                ),
                                cx,
                            )),
                    )
                    .child(
                        property_panel("Kernel boot configuration", cx)
                            .child(property_row(
                                "Image state",
                                if instance.image.present {
                                    "Loaded"
                                } else {
                                    "Not loaded"
                                }
                                .to_owned(),
                                cx,
                            ))
                            .child(property_row(
                                "Lifecycle state",
                                instance_state_label(instance.state),
                                cx,
                            ))
                            .child(property_row(
                                "Console transport",
                                if instance.state == InstanceState::Active {
                                    "MKTTY available"
                                } else {
                                    "Available when active"
                                }
                                .to_owned(),
                                cx,
                            )),
                    ),
            )
            .child(
                property_panel("Assigned devices", cx).child(property_row(
                    "Device IDs",
                    if instance.resources.device_ids.is_empty() {
                        "No devices assigned".to_owned()
                    } else {
                        instance.resources.device_ids.join(", ")
                    },
                    cx,
                )),
            )
            .child(
                Alert::info(
                    ("manage-note", id.0 as usize),
                    "Resource changes are validated against the authoritative host topology before they are applied.",
                )
                .title("Fail-closed resource management"),
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
        let operations = &snapshot.host.operations;
        let transactions = &snapshot.host.transactions;
        let operation_failures = operations
            .iter()
            .filter(|item| {
                matches!(
                    item.state,
                    OperationState::Failed | OperationState::Indeterminate
                )
            })
            .count();
        let transaction_failures = transactions
            .iter()
            .filter(|item| item.state == TransactionState::Failed)
            .count();
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .min_h(px(38.))
                    .gap_5()
                    .px_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary.opacity(0.16))
                    .child(activity_count("Operations", operations.len(), cx))
                    .child(activity_count(
                        "Resource transactions",
                        transactions.len(),
                        cx,
                    ))
                    .child(activity_count(
                        "Needs attention",
                        operation_failures + transaction_failures,
                        cx,
                    )),
            )
            .child(operation_table(operations, cx))
            .child(transaction_table(transactions, cx))
    }

    fn instance_monitor(
        &self,
        snapshot: &ManagementSnapshot,
        id: InstanceId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let operations = snapshot
            .host
            .operations
            .iter()
            .filter(|operation| operation_affects_instance(operation, id))
            .collect::<Vec<_>>();
        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(("instance-summary", id.0 as usize))
                            .outline()
                            .label("Summary")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.action_error = None;
                                this.model.select_instance(Some(id));
                                this.model.navigate(Section::Instances);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("refresh-instance-monitor", id.0 as usize))
                            .outline()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .child(
                summary_section("Performance", cx).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Historical instance telemetry is not available from this host."),
                ),
            )
            .child(
                summary_section("Tasks", cx)
                    .children(
                        operations
                            .iter()
                            .map(|operation| operation_row(operation, cx)),
                    )
                    .children(operations.is_empty().then(|| {
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No tasks are associated with this instance.")
                    })),
            )
    }

    fn global_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(38.))
            .px_3()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.22))
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::Cpu).size_4())
                    .child(div().font_semibold().child("Kernmux"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Host Client"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Local administration"),
            )
    }

    fn recent_tasks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (operations, transactions) = match self.model.data() {
            DataState::Ready(snapshot) => (
                snapshot.host.operations.as_slice(),
                snapshot.host.transactions.as_slice(),
            ),
            _ => (&[][..], &[][..]),
        };
        let activity_count = operations.len() + transactions.len();
        let operation_rows = operations.len().min(3);
        let transaction_rows = 3usize.saturating_sub(operation_rows);
        let collapsed = self.recent_tasks_collapsed;
        v_flex()
            .w_full()
            .h(if collapsed { px(34.) } else { px(142.) })
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h(px(34.))
                    .px_4()
                    .justify_between()
                    .bg(cx.theme().secondary.opacity(0.18))
                    .child(div().text_sm().font_semibold().child("Recent Activity"))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{activity_count} entries")),
                            )
                            .child(
                                Button::new("toggle-recent-tasks")
                                    .label(if collapsed { "Show" } else { "Hide" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.recent_tasks_collapsed = !this.recent_tasks_collapsed;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .children((!collapsed).then(|| {
                v_flex()
                    .child(
                        h_flex()
                            .h(px(26.))
                            .px_3()
                            .bg(cx.theme().secondary.opacity(0.22))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(div().w(px(180.)).child("Activity"))
                            .child(div().w(px(180.)).child("Scope"))
                            .child(div().flex_1().child("Generation / Started"))
                            .child(div().w(px(110.)).child("Result")),
                    )
                    .children(
                        operations
                            .iter()
                            .rev()
                            .take(operation_rows)
                            .map(|operation| {
                                h_flex()
                                    .h(px(27.))
                                    .px_3()
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .text_xs()
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .font_medium()
                                            .child(operation_label(operation.kind)),
                                    )
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(operation_target(operation)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(display_timestamp(&operation.created_at)),
                                    )
                                    .child(div().w(px(110.)).child(operation_tag(operation.state)))
                            }),
                    )
                    .children(
                        transactions
                            .iter()
                            .rev()
                            .take(transaction_rows)
                            .map(|transaction| {
                                let generation = match (
                                    transaction.generation_before,
                                    transaction.generation_after,
                                ) {
                                    (Some(before), Some(after)) => {
                                        format!("{} → {}", before.0, after.0)
                                    }
                                    _ => "Not reported".to_owned(),
                                };
                                h_flex()
                                    .h(px(27.))
                                    .px_3()
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .text_xs()
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .font_medium()
                                            .child(format!("Transaction {}", transaction.id)),
                                    )
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Resource allocation"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(generation),
                                    )
                                    .child(
                                        div().w(px(110.)).child(transaction_tag(transaction.state)),
                                    )
                            }),
                    )
                    .children((activity_count == 0).then(|| {
                        div()
                            .px_3()
                            .py_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No host activity is available")
                    }))
            }))
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

fn this_section_is_instance_monitor(
    section: Section,
    selected: Option<InstanceId>,
    id: InstanceId,
) -> bool {
    section == Section::Operations && selected == Some(id)
}

fn instance_table_header(cx: &App) -> gpui::Div {
    h_flex()
        .w_full()
        .min_h(px(36.))
        .px_3()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().secondary.opacity(0.22))
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(div().w(px(260.)).child("Instance"))
        .child(div().w(px(140.)).child("State"))
        .child(div().w(px(90.)).child("CPU"))
        .child(div().w(px(130.)).child("Memory"))
        .child(div().flex_1().child("Kernel image"))
        .child(div().w(px(72.)).child("ID"))
}

fn summary_panel(title: &str, cx: &App) -> gpui::Div {
    v_flex()
        .min_h(px(190.))
        .gap_3()
        .p_4()
        .border_1()
        .border_color(cx.theme().border)
        .child(div().font_semibold().child(title.to_owned()))
}

fn property_panel(title: &str, cx: &App) -> gpui::Div {
    v_flex()
        .w_full()
        .border_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .h(px(30.))
                .px_3()
                .bg(cx.theme().secondary.opacity(0.28))
                .border_b_1()
                .border_color(cx.theme().border)
                .text_sm()
                .font_semibold()
                .child(title.to_owned()),
        )
}

fn property_row(label: &str, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .min_h(px(30.))
        .border_t_1()
        .border_color(cx.theme().border.opacity(0.65))
        .text_sm()
        .child(
            div()
                .w(px(190.))
                .self_stretch()
                .flex()
                .items_center()
                .px_3()
                .bg(cx.theme().secondary.opacity(0.16))
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(div().min_w_0().flex_1().px_3().child(value))
}

fn utilization_row(
    label: &str,
    value: String,
    percent_value: f32,
    accessibility_label: &str,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(cx.theme().border.opacity(0.65))
        .child(
            h_flex()
                .justify_between()
                .text_sm()
                .child(label.to_owned())
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(value),
                ),
        )
        .child(
            Progress::new(accessibility_label.to_owned())
                .value(percent_value)
                .accessibility_label(accessibility_label.to_owned()),
        )
}

fn summary_section(title: &str, cx: &App) -> gpui::Div {
    v_flex()
        .w_full()
        .gap_3()
        .p_4()
        .border_1()
        .border_color(cx.theme().border)
        .child(div().font_semibold().child(title.to_owned()))
}

fn instance_state_label(state: InstanceState) -> String {
    match state {
        InstanceState::Active => "Active",
        InstanceState::Loaded => "Loaded",
        InstanceState::Ready => "Ready",
        InstanceState::Absent => "Absent",
        InstanceState::Unknown => "Unknown",
    }
    .to_owned()
}

fn hardware_ids(ids: &[u32]) -> String {
    if ids.is_empty() {
        "None".to_owned()
    } else {
        ids.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn operation_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::InitializeResourcePool => "Configure resource pool",
        OperationKind::ReleaseResourcePool => "Release resource pool",
        OperationKind::CreateInstance => "Create instance",
        OperationKind::UpdateInstance => "Update instance",
        OperationKind::LoadInstance => "Load kernel image",
        OperationKind::StartInstance => "Start instance",
        OperationKind::StopInstance => "Stop instance",
        OperationKind::UnloadInstance => "Unload kernel image",
        OperationKind::DeleteInstance => "Delete instance",
        OperationKind::OpenConsole => "Open console",
        OperationKind::ImportImage => "Register image",
        OperationKind::Unknown => "Host operation",
    }
}

fn operation_target(operation: &Operation) -> String {
    operation.affected_resources.first().map_or_else(
        || "Host".to_owned(),
        |resource| match resource.kind {
            ResourceKind::Host => "Host".to_owned(),
            ResourceKind::ResourcePool => "Resource pool".to_owned(),
            ResourceKind::Instance => format!("Instance {}", resource.id),
            ResourceKind::Device => format!("Device {}", resource.id),
            ResourceKind::Console => format!("Console {}", resource.id),
            ResourceKind::Image => format!("Image {}", resource.id),
            ResourceKind::Unknown => resource.id.clone(),
        },
    )
}

fn operation_affects_instance(operation: &Operation, id: InstanceId) -> bool {
    let id = id.0.to_string();
    operation
        .affected_resources
        .iter()
        .any(|resource| resource.kind == ResourceKind::Instance && resource.id == id)
}

fn display_timestamp(value: &str) -> String {
    value.split_once('T').map_or_else(
        || value.to_owned(),
        |(_, time)| {
            time.split('.')
                .next()
                .unwrap_or(time)
                .trim_end_matches('Z')
                .to_owned()
        },
    )
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

fn activity_count(label: &str, value: usize, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(div().text_lg().font_semibold().child(value.to_string()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
}

fn operation_table(operations: &[Operation], cx: &App) -> gpui::Div {
    v_flex()
        .w_full()
        .border_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .h(px(32.))
                .px_3()
                .bg(cx.theme().secondary.opacity(0.28))
                .font_semibold()
                .child("Operations"),
        )
        .child(
            h_flex()
                .h(px(28.))
                .px_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(div().w(px(210.)).child("Task"))
                .child(div().w(px(170.)).child("Target"))
                .child(div().w(px(110.)).child("Started"))
                .child(div().w(px(130.)).child("Generation"))
                .child(div().flex_1().child("Result")),
        )
        .children(operations.iter().map(|operation| {
            let generation = operation.observed_generation.map_or_else(
                || format!("{} → —", operation.expected_generation.0),
                |observed| format!("{} → {}", operation.expected_generation.0, observed.0),
            );
            let result = operation.error.as_ref().map_or_else(
                || operation_state_label(operation.state).to_owned(),
                |error| error.message.clone(),
            );
            h_flex()
                .min_h(px(34.))
                .px_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_sm()
                .child(
                    div()
                        .w(px(210.))
                        .min_w_0()
                        .font_medium()
                        .child(operation_label(operation.kind)),
                )
                .child(
                    div()
                        .w(px(170.))
                        .min_w_0()
                        .text_color(cx.theme().muted_foreground)
                        .child(operation_target(operation)),
                )
                .child(
                    div()
                        .w(px(110.))
                        .text_color(cx.theme().muted_foreground)
                        .child(display_timestamp(&operation.created_at)),
                )
                .child(div().w(px(130.)).child(generation))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(operation_tag(operation.state))
                        .child(div().min_w_0().text_xs().child(result)),
                )
        }))
        .children(operations.is_empty().then(|| {
            div()
                .px_3()
                .py_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No asynchronous operations are retained by this daemon session.")
        }))
}

fn transaction_table(transactions: &[Transaction], cx: &App) -> gpui::Div {
    v_flex()
        .w_full()
        .border_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .h(px(32.))
                .px_3()
                .bg(cx.theme().secondary.opacity(0.28))
                .font_semibold()
                .child("Resource Transactions"),
        )
        .child(
            h_flex()
                .h(px(28.))
                .px_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(div().w(px(150.)).child("Transaction"))
                .child(div().w(px(130.)).child("State"))
                .child(div().w(px(170.)).child("Generation"))
                .child(div().flex_1().child("Diagnostic")),
        )
        .children(transactions.iter().map(|transaction| {
            let generations = match (transaction.generation_before, transaction.generation_after) {
                (Some(before), Some(after)) => format!("{} → {}", before.0, after.0),
                (Some(before), None) => format!("{} → —", before.0),
                (None, Some(after)) => format!("— → {}", after.0),
                (None, None) => "Not reported".to_owned(),
            };
            let diagnostic = transaction.diagnostics.first().map_or_else(
                || {
                    if transaction.state == TransactionState::Failed {
                        "No diagnostic reported by the kernel".to_owned()
                    } else {
                        "No diagnostic".to_owned()
                    }
                },
                |item| format!("{}: {}", item.code, item.message),
            );
            h_flex()
                .min_h(px(36.))
                .px_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_sm()
                .bg(if transaction.state == TransactionState::Failed {
                    cx.theme().danger.opacity(0.06)
                } else {
                    cx.theme().background
                })
                .child(
                    div()
                        .w(px(150.))
                        .font_medium()
                        .child(format!("Transaction {}", transaction.id)),
                )
                .child(div().w(px(130.)).child(transaction_tag(transaction.state)))
                .child(div().w(px(170.)).child(generations))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(diagnostic),
                )
        }))
        .children(transactions.is_empty().then(|| {
            div()
                .px_3()
                .py_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No resource transactions are exposed by the running kernel.")
        }))
}

fn operation_row(operation: &Operation, cx: &App) -> impl IntoElement {
    card(cx)
        .child(
            h_flex()
                .justify_between()
                .child(
                    v_flex()
                        .gap_1()
                        .child(div().font_medium().child(operation_label(operation.kind)))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(operation_target(operation)),
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
                .child(format!(
                    "Started {}",
                    display_timestamp(&operation.created_at)
                )),
        )
}

fn empty_state(title: &str, detail: &str, cx: &App) -> impl IntoElement {
    v_flex()
        .w_full()
        .items_center()
        .justify_center()
        .gap_2()
        .py_6()
        .border_t_1()
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

fn operation_state_label(state: OperationState) -> &'static str {
    match state {
        OperationState::Succeeded => "Succeeded",
        OperationState::Running => "Running",
        OperationState::Queued => "Queued",
        OperationState::Failed => "Failed",
        OperationState::Indeterminate => "Outcome unknown",
        OperationState::Cancelled => "Cancelled",
        OperationState::Unknown => "Unknown",
    }
}

fn transaction_tag(state: TransactionState) -> Tag {
    match state {
        TransactionState::Applied => Tag::success().outline().child("Applied"),
        TransactionState::Planned => Tag::info().outline().child("Planned"),
        TransactionState::RolledBack => Tag::warning().outline().child("Rolled back"),
        TransactionState::Failed => Tag::danger().outline().child("Failed"),
        TransactionState::Unknown => Tag::warning().outline().child("Unknown"),
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

fn percent_usize(value: usize, total: usize) -> f32 {
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
