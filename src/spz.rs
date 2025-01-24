
use ::core::f32;
use std::{
    rc::Rc,
    cell::RefCell,
};
use web_sys::{
    Worker,
    WorkerOptions,
    WorkerType,
    MessageEvent,
};
use wasm_bindgen::prelude::*;
use js_sys::{Object, JsString, Number, Reflect, Float32Array, Boolean};
use bus::{Bus, BusReader};

use crate::log; // macro import
use crate::scene::SerializedSplat2;
use crate::utils::*;


#[derive(Debug, Clone)]
pub struct GaussianCloud {
  pub num_points: i32,
  pub sh_degree: i32,
  pub antialiased: bool,
  pub positions: Vec<f32>,
  pub scales: Vec<f32>,
  pub rotations: Vec<f32>,
  pub alphas: Vec<f32>,
  pub colors: Vec<f32>,
  pub sh: Vec<f32>,
}
impl From<js_sys::Object> for GaussianCloud {
    fn from(gaussian_cloud: js_sys::Object) -> GaussianCloud {
        let num_points: i32 = Reflect::get(&gaussian_cloud, &JsValue::from_str("numPoints"))
            .unwrap()
            .dyn_into::<Number>()
            .unwrap()
            .value_of() as i32;
        log!("From for GaussianCloud: num_points={}", num_points);

        let sh_degree: i32 = Reflect::get(&gaussian_cloud, &JsValue::from_str("shDegree"))
            .unwrap()
            .dyn_into::<Number>()
            .unwrap()
            .value_of() as i32;
        log!("From for GaussianCloud: sh_degree={}", sh_degree);

        let antialiased: bool = Reflect::get(&gaussian_cloud, &JsValue::from_str("antialiased"))
            .unwrap()
            .dyn_into::<Boolean>()
            .unwrap()
            .value_of();
        log!("From for GaussianCloud: antialiased={}", antialiased);

        let positions = Reflect::get(&gaussian_cloud, &JsValue::from_str("positions")).unwrap();
        let positions = Float32Array::new(&positions);
        let positions: Vec<f32> = positions.to_vec();
        log!("From for GaussianCloud: positions.len()={}", positions.len());

        let scales = Reflect::get(&gaussian_cloud, &JsValue::from_str("scales")).unwrap();
        let scales = Float32Array::new(&scales);
        let scales: Vec<f32> = scales.to_vec();
        log!("From for GaussianCloud: scales.len()={}", scales.len());

        let rotations = Reflect::get(&gaussian_cloud, &JsValue::from_str("rotations")).unwrap();
        let rotations = Float32Array::new(&rotations);
        let rotations: Vec<f32> = rotations.to_vec();
        log!("From for GaussianCloud: rotations.len()={}", rotations.len());

        let alphas = Reflect::get(&gaussian_cloud, &JsValue::from_str("alphas")).unwrap();
        let alphas = Float32Array::new(&alphas);
        let alphas: Vec<f32> = alphas.to_vec();
        log!("From for GaussianCloud: alphas.len()={}", alphas.len());

        let colors = Reflect::get(&gaussian_cloud, &JsValue::from_str("colors")).unwrap();
        //let colors = Uint8Array::new(&colors);
        let colors = Float32Array::new(&colors);
        let colors: Vec<f32> = colors.to_vec();
        log!("From for GaussianCloud: colors.len()={}", colors.len());

        let sh = Reflect::get(&gaussian_cloud, &JsValue::from_str("sh")).unwrap();
        let sh = Float32Array::new(&sh);
        let sh: Vec<f32> = sh.to_vec();
        log!("From for GaussianCloud: sh.len()={}", sh.len());

        GaussianCloud {
            num_points,
            sh_degree,
            antialiased,
            positions,
            scales,
            rotations,
            alphas,
            colors,
            sh
        }
    }
}
impl GaussianCloud {
    pub fn create_serialized_splat_vec(&self) -> Vec<SerializedSplat2> {
        let num_points = self.num_points as usize;
        if num_points == 0 {
            log!("GaussianCloud::create_serialized_splat_vec(): WARNING: num_points is 0.");
        }
        let mut serialized_splats = vec![SerializedSplat2::default(); num_points];
        for i in 0..num_points {
            let splat = &mut serialized_splats[i];

            splat.position = [
                self.positions[i*3 + 0],
                self.positions[i*3 + 1],
                self.positions[i*3 + 2],
            ];

            splat.scale = [
                self.scales[i*3 + 0],
                self.scales[i*3 + 1],
                self.scales[i*3 + 2],
            ];

            splat.rotation = [
                self.rotations[i*4 + 0],
                self.rotations[i*4 + 1],
                self.rotations[i*4 + 2],
                self.rotations[i*4 + 3],
            ];

            splat.alpha = self.alphas[i];

            let color = [
                self.colors[i*3 + 0],
                self.colors[i*3 + 1],
                self.colors[i*3 + 2],
            ];
            let sh = &self.sh[(i*45)+0..(i*45)+45];
            let mut concatenated = Vec::<f32>::with_capacity(color.len() + sh.len());
            concatenated.extend_from_slice(&color);
            concatenated.extend_from_slice(&sh);
            splat.color = *concatenated.as_array().unwrap();
        }

        serialized_splats
    }
}


pub struct Spz {
    worker_handle: Option<Worker>,
    rx_loaded: Option<BusReader<GaussianCloud>>,
}
impl Spz {
    pub fn new() -> Self {
        Self {
            worker_handle: None,
            rx_loaded: None,
        }
    }


    pub fn init(
        &mut self,
    ) {
        let mut bus_loaded = Bus::<GaussianCloud>::new(1);
        let rx_loaded = bus_loaded.add_rx();
        let bus_loaded_rc = Rc::new(RefCell::new(bus_loaded));

        {
            let worker_handle = match Worker::new_with_options(
                "/spz.js",
                WorkerOptions::new().type_(WorkerType::Module)
            ) {
                Ok(worker) => {
                    log!("Spz::init(): Worker created successfully.");
                    worker
                },
                Err(e) => {
                    panic!("Spz::init(): Failed to create worker: {:?}", e);
                }
            };

            let callback_handle = self.onmessage(
                bus_loaded_rc,
            );
            worker_handle.set_onmessage(Some(callback_handle.as_ref().unchecked_ref()));

            callback_handle.forget(); // avoid being dropped prematurely

            self.worker_handle = Some(worker_handle);
        }

        self.rx_loaded = Some(rx_loaded);
    }


    /// Sends data to Worker
    pub fn post2worker(
        &mut self,
        type_str: &str,
        url: Option<String>,
    ) {
        if let Some(worker_handle) = &self.worker_handle {
            let msg = js_sys::Object::new();
            js_sys::Reflect::set(&msg, &"type".into(), &JsValue::from_str(type_str)).unwrap();

            // create message depending on type
            match type_str {
                "load" => {
                    if let Some(url) = url {
                        log!("Spz::post2worker(): type=load, url={}", url);
                        js_sys::Reflect::set(&msg, &"url".into(), &JsValue::from_str(&url)).unwrap();
                    } else {
                        log!("Spz::post2worker(): ERROR: url is None.");
                        return;
                    }
                },

                _ => {
                    log!("Spz::post2worker(): ERROR: Unknown type_str: {}", type_str);
                    return;
                },
            }

            // send message to worker
            worker_handle.post_message(&msg).expect("Spz::post2worker(): ERROR: Failed to post message to worker.");
        } else {
            log!("Spz::post2worker(): WARNING: worker_handle is None.");
        }
    }


    fn onmessage(
        &self,
        bus_loaded: Rc<RefCell<Bus<GaussianCloud>>>,
    ) -> Closure<dyn FnMut(MessageEvent) + 'static> {
        let callback = Closure::wrap(Box::new(move |event: MessageEvent| {
            let data: Object  = event
                .data()
                .dyn_into()
                .unwrap();

            // status string
            let status: String = js_sys::Reflect::get(&data, &JsValue::from_str("status"))
                .unwrap()
                .dyn_into::<JsString>()
                .unwrap()
                .as_string()
                .unwrap_or_else(|| {
                    String::from("Spz::onmessage(): ERROR: Failed to convert status")
                });

            if status.starts_with("loaded") {
                log!("Spz::onmessage(): loaded");

                // Extract gaussianCloud object
                let gaussian_cloud: js_sys::Object = Reflect::get(&data, &JsValue::from_str("gaussianCloud"))
                    .unwrap()
                    .into();
                let gc: GaussianCloud = gaussian_cloud.into();

                //////////////////////////////////
                // non-blocking (i.e., no atomic.wait)
                let mut bus_loaded = bus_loaded.as_ref().borrow_mut();
                let _ = bus_loaded.try_broadcast(gc);
                //////////////////////////////////

            } else {
            }
        }) as Box<dyn FnMut(_)>);

        callback
    }
}


/// Loads spz. Blocks until spz is loaded.
pub async fn load_spz(spz: &mut Spz, buffer: Vec<u8>) -> Vec<SerializedSplat2> {
    log!("load_spz(): buffer.len()={}", buffer.len());

    if spz.rx_loaded.is_none() {
        unreachable!("load_spz(): ERROR: spz.rx_loaded is None");
    }
    if buffer.is_empty() {
        unreachable!("load_spz(): ERROR: buffer is empty");
    }

    let mut serialized_splats = Vec::<SerializedSplat2>::new();
    if let Ok(url) = create_url_byte_array(buffer) {
        spz.post2worker("load", Some(url));
        if let Some(rx_loaded) = spz.rx_loaded.as_mut() {

            // no direct blocking available in wasm (ie. rx_loaded.recv())
            let mut i = 0;
            loop {
                if let Ok(gc) = rx_loaded.try_recv() {
                    serialized_splats = gc.create_serialized_splat_vec();
                    return serialized_splats;
                }

                sleep_js(1000).await;
                i += 1;
                if i > 33 {
                    unreachable!("load_spz(): ERROR: timed out");
                }
            }
        }
    } else {
        unreachable!("load_spz(): ERROR: create_url_byte_array() failed");
    }

    serialized_splats
}
