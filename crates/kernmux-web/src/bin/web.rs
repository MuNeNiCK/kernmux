#![cfg_attr(target_arch = "wasm32", no_main)]

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::OnceCell;

    use kernmux_api::v1::{HostSnapshot, ImageArtifact, Response as ApiResponse};
    use kernmux_ui_model::ManagementSnapshot;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{Event, Headers, MediaQueryList, Request, RequestInit, Response};

    type ReducedMotionListener = (MediaQueryList, Closure<dyn FnMut(Event)>);

    thread_local! {
        static APPLICATION: OnceCell<gpui::ApplicationHandle> = const { OnceCell::new() };
        static REDUCED_MOTION: OnceCell<ReducedMotionListener> = const { OnceCell::new() };
    }

    #[wasm_bindgen::prelude::wasm_bindgen(start)]
    pub fn start() {
        gpui_platform::web_init();
        let application =
            gpui_platform::application().run_embedded(kernmux_web::open_management_shell);
        APPLICATION.with(|slot| {
            assert!(slot.set(application).is_ok(), "application already started");
        });
        start_reduced_motion_preference();
        spawn_local(async {
            match load_management_snapshot().await {
                Ok(snapshot) => APPLICATION.with(|slot| {
                    if let Some(application) = slot.get() {
                        application
                            .update(|cx| kernmux_web::install_management_snapshot(snapshot, cx));
                    }
                }),
                Err(message) => APPLICATION.with(|slot| {
                    if let Some(application) = slot.get() {
                        application.update(|cx| kernmux_web::fail_management_shell(message, cx));
                    }
                }),
            }
        });
    }

    fn start_reduced_motion_preference() {
        let Some(query) = web_sys::window().and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        }) else {
            return;
        };
        apply_reduced_motion(query.matches());
        let observed = query.clone();
        let listener = Closure::new(move |_event: Event| apply_reduced_motion(observed.matches()));
        if query
            .add_event_listener_with_callback("change", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            REDUCED_MOTION.with(|slot| {
                let _ = slot.set((query, listener));
            });
        }
    }

    fn apply_reduced_motion(reduce: bool) {
        APPLICATION.with(|slot| {
            if let Some(application) = slot.get() {
                application.update(|cx| cx.set_reduce_motion(reduce));
            }
        });
    }

    async fn load_management_snapshot() -> Result<ManagementSnapshot, String> {
        let token = consume_fragment_token()?;
        let host = fetch_result::<HostSnapshot>("/api/1.0", &token).await?;
        let images = fetch_result::<Vec<ImageArtifact>>("/api/1.0/images", &token).await?;
        Ok(ManagementSnapshot { host, images })
    }

    fn consume_fragment_token() -> Result<String, String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let hash = window
            .location()
            .hash()
            .map_err(|_| "management credential is unavailable")?;
        let token = hash.strip_prefix("#token=").unwrap_or_default();
        if token.len() < 32 || token.len() > 512 || token.chars().any(char::is_whitespace) {
            return Err("Open this host with a valid management credential.".into());
        }
        window
            .history()
            .and_then(|history| history.replace_state_with_url(&JsValue::NULL, "", Some("/")))
            .map_err(|_| {
                "management credential could not be removed from the address bar".to_owned()
            })?;
        Ok(token.to_owned())
    }

    async fn fetch_result<T: serde::de::DeserializeOwned>(
        path: &str,
        token: &str,
    ) -> Result<T, String> {
        let headers = Headers::new().map_err(|_| "request headers are unavailable")?;
        headers
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(|_| "management credential is invalid")?;
        headers
            .set("Accept", "application/json")
            .map_err(|_| "request headers are unavailable")?;
        let options = RequestInit::new();
        options.set_method("GET");
        options.set_headers(&headers);
        let request = Request::new_with_str_and_init(path, &options)
            .map_err(|_| "management request could not be created")?;
        let response = JsFuture::from(
            web_sys::window()
                .ok_or_else(|| "browser window is unavailable".to_owned())?
                .fetch_with_request(&request),
        )
        .await
        .map_err(|_| "management gateway is unreachable")?
        .dyn_into::<Response>()
        .map_err(|_| "management gateway returned an invalid response")?;
        let text = JsFuture::from(
            response
                .text()
                .map_err(|_| "management response is unreadable")?,
        )
        .await
        .map_err(|_| "management response is unreadable")?
        .as_string()
        .ok_or_else(|| "management response is not text".to_owned())?;
        let envelope: ApiResponse<T> = serde_json::from_str(&text)
            .map_err(|_| "management response does not match the API contract")?;
        match envelope {
            ApiResponse::Result { data, .. } if response.ok() => Ok(data),
            ApiResponse::Error { error } => Err(error.message),
            ApiResponse::Accepted { .. } | ApiResponse::Result { .. } => {
                Err("management response status is inconsistent".into())
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("the Kernmux web client must be built for wasm32-unknown-unknown");
}
