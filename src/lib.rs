#![allow(unused_assignments, unused_variables, dead_code)]
#![feature(slice_as_array)]

use wasm_bindgen::prelude::*;

mod renderer;
mod scene;
pub mod scene_graph;
mod spz;
mod utils;

#[wasm_bindgen(start)]
pub fn dummy_main() {}

#[wasm_bindgen]
pub async fn run() {
    utils::set_panic_hook();
    renderer::main().await;
}
