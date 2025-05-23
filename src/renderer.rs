#[allow(unused_imports)]
use std::{
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    rc::Rc,
    cell::RefCell,
};

//use parking_lot::Mutex;
use three_d::*;
use wasm_thread as thread;
use bus::{Bus, BusReader};
use num_format::{Locale, ToFormattedString};

use crate::log; // macro import
use crate::utils::*;
use crate::scene::*;


#[derive(PartialEq)]
enum TdCameraControl { Orbit, Fly }


/// Re-implementation of three_d::OrbitControl to add right mouse button control
pub struct OrbitControl2 {
    control: CameraControl,
}
impl OrbitControl2 {
    /// Creates a new orbit control with the given target and minimum and maximum distance to the target.
    pub fn new(target: Vec3, min_distance: f32, max_distance: f32) -> Self {
        Self {
            control: CameraControl {
                left_drag_horizontal: CameraAction::OrbitLeft { target, speed: 0.1 },
                left_drag_vertical: CameraAction::OrbitUp { target, speed: 0.1 },
                scroll_vertical: CameraAction::Zoom {
                    min: min_distance,
                    max: max_distance,
                    speed: 0.001,
                    target,
                },
                right_drag_horizontal: CameraAction::Left { speed: 0.01 },
                right_drag_vertical: CameraAction::Up { speed: 0.01 },
                ..Default::default()
            },
        }
    }

    /// Handles the events. Must be called each frame.
    pub fn handle_events(&mut self, camera: &mut Camera, events: &mut [Event]) -> bool {

        // need to re-calculate the change so as to translate the target for orbit
        let mut change = Vec3::zero();
        for event in events.iter() {
            match event {
                Event::MouseMotion {
                    delta,
                    button,
                    handled,
                    ..
                } => {
                    if let Some(b) = button {
                        if let MouseButton::Right = b {
                            if let CameraAction::Left { speed } = &self.control.right_drag_horizontal {
                                change += -camera.right_direction() * delta.0 * (*speed);
                            }
                            if let CameraAction::Up { speed } = &self.control.right_drag_vertical {
                                let right = camera.right_direction();
                                let up = right.cross(camera.view_direction());
                                change += up * delta.1 * (*speed);
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        if let CameraAction::Zoom { speed, target, .. } = &mut self.control.scroll_vertical {
            let x = target.distance(*camera.position());
            *speed = 0.001 * x + 0.001;
            *target += change;
        }
        if let CameraAction::OrbitLeft { speed, target } = &mut self.control.left_drag_horizontal {
            let x = target.distance(*camera.position());
            *speed = 0.01 * x + 0.001;
            *target += change;
        }
        if let CameraAction::OrbitUp { speed, target } = &mut self.control.left_drag_vertical {
            let x = target.distance(*camera.position());
            *speed = 0.01 * x + 0.001;
            *target += change;
        }

        self.control.handle_events(camera, events)
    }
}


#[allow(unused_mut)]
fn launch_sorter_thread(
    scene: Arc<Scene>,
    mut rx_buffer: BusReader<Vec<u8>>,
    mut rx_vp: BusReader<Mat4>,
    mut bus_depth: Bus<Vec<u32>>,
    cpu_cores: usize,
    mut bus_time: Bus<f64>,
) -> thread::JoinHandle<()> {
    // launch another thread for view-dependent splat sorting
    let thread_handle = thread::spawn({
        let mut scene = scene.clone();

        move || loop {
            // receive splat binary buffer from async JS worker callback
            #[cfg(feature = "async_splat_stream")]
            if let Ok(buffer) = rx_buffer.try_recv() {
                /*
                FIXME: scene buffer needs to be duplicated here
                since Arc<Scene> does not have an interior mutability without a mutex
                (and mutex is not allowed in wasm main thread)
                */
                let mut s = Scene::new();
                s.buffer = buffer;
                s.splat_count = s.buffer.len() / 32; // 32bytes per splat
                //s.generate_texture(); // texture is created instead in render loop in main thread
                scene = Arc::new(s);
            }

            // receive view proj matrix from main thread
            if let Ok(view_proj) = rx_vp.try_recv() {
                let view_proj_slice = &[
                    view_proj[0][0], view_proj[0][1], view_proj[0][2], view_proj[0][3],
                    view_proj[1][0], view_proj[1][1], view_proj[1][2], view_proj[1][3],
                    view_proj[2][0], view_proj[2][1], view_proj[2][2], view_proj[2][3],
                    view_proj[3][0], view_proj[3][1], view_proj[3][2], view_proj[3][3]
                ];
                let start =  get_time_milliseconds();
                Scene::sort(&scene, view_proj_slice, &mut bus_depth, cpu_cores);
                let sort_time = get_time_milliseconds() - start;
                //////////////////////////////////
                // non-blocking (i.e., no atomic.wait)
                let _ = bus_time.try_broadcast(sort_time);
                //////////////////////////////////
            }
        }
    });

    thread_handle
}


/*
#[allow(unused_mut)]
fn launch_sorter_thread2(
    mut rx_buffer: BusReader<Vec<u8>>,
    mut rx_vp: BusReader<Mat4>,
    mut bus_depth: Bus<Vec<u32>>,
    cpu_cores: usize,
    mut bus_time: Bus<f64>,
) -> thread::JoinHandle<()> {
    // launch another thread for view-dependent splat sorting
    let thread_handle = thread::spawn({
        let mut scene = Scene::new();

        move || loop {
            // receive splat chunk from async JS worker callback
            #[cfg(feature = "async_splat_stream")]
            if let Ok(chunk) = rx_buffer.try_recv() {
                scene.buffer.extend(chunk);
                scene.splat_count = scene.buffer.len() / 32; // 32bytes per splat
            }

            // receive view proj matrix from main thread
            if let Ok(view_proj) = rx_vp.try_recv() {
                let view_proj_slice = &[
                    view_proj[0][0], view_proj[0][1], view_proj[0][2], view_proj[0][3],
                    view_proj[1][0], view_proj[1][1], view_proj[1][2], view_proj[1][3],
                    view_proj[2][0], view_proj[2][1], view_proj[2][2], view_proj[2][3],
                    view_proj[3][0], view_proj[3][1], view_proj[3][2], view_proj[3][3]
                ];
                let start =  get_time_milliseconds();
                Scene::sort2(&scene, view_proj_slice, &mut bus_depth, cpu_cores);
                let sort_time = get_time_milliseconds() - start;
                //////////////////////////////////
                // non-blocking (i.e., no atomic.wait)
                let _ = bus_time.try_broadcast(sort_time);
                //////////////////////////////////
            }
        }
    });

    thread_handle
}
*/


fn create_glsl_program(
    gl: &Context,
    vs_file: &str,
    fs_file: &str,
    error_flag: &Arc<AtomicBool>,
    error_msg: &Arc<Mutex<String>>
) -> context::Program {
    unsafe {
        let vert_shader = gl.create_shader(context::VERTEX_SHADER)
            .expect("Failed creating vertex shader");
        let frag_shader = gl.create_shader(context::FRAGMENT_SHADER)
            .expect("Failed creating fragment shader");

        gl.shader_source(vert_shader, vs_file);
        gl.shader_source(frag_shader, fs_file);
        gl.compile_shader(vert_shader);
        gl.compile_shader(frag_shader);

        let id = gl.create_program()
            .expect("Failed creating program");

        gl.attach_shader(id, vert_shader);
        gl.attach_shader(id, frag_shader);
        gl.link_program(id);

        if !gl.get_program_link_status(id) {
            let log = gl.get_shader_info_log(vert_shader);
            if !log.is_empty() {
                set_error_for_egui(
                    error_flag, error_msg,
                    format!("ERROR: gl.get_program_link_status(): {}", log)
                );
            }
            let log = gl.get_shader_info_log(frag_shader);
            if !log.is_empty() {
                set_error_for_egui(
                    error_flag, error_msg,
                    format!("ERROR: gl.get_program_link_status(): {}", log)
                );
            }
            let log = gl.get_program_info_log(id);
            if !log.is_empty() {
                set_error_for_egui(
                    error_flag, error_msg,
                    format!("ERROR: gl.get_program_link_status(): {}", log)
                );
            }
            //unreachable!();
        } else {
            gl.detach_shader(id, vert_shader);
            gl.detach_shader(id, frag_shader);
            gl.delete_shader(vert_shader);
            gl.delete_shader(frag_shader);
        }

        return id;
    }
}


struct SplatGLSL {
    program: Option<context::Program>,
    u_projection: Option<context::UniformLocation>,
    u_viewport: Option<context::UniformLocation>,
    u_focal: Option<context::UniformLocation>,
    u_htan_fov: Option<context::UniformLocation>,
    u_view: Option<context::UniformLocation>,
    u_cam_pos: Option<context::UniformLocation>,
    u_splat_scale: Option<context::UniformLocation>,

    vertex_buffer: Option<context::WebBufferKey>,
    a_position: u32,

    texture: Option<context::WebTextureKey>,
    u_splat_texture: Option<context::UniformLocation>,

    index_buffer: Option<context::WebBufferKey>,
    a_index: u32,
}
impl SplatGLSL {
    const VERT_SHADER: &'static str = include_str!("gsplat.vert");
    const FRAG_SHADER: &'static str = include_str!("gsplat.frag");


    pub fn new() -> Self {
        Self {
            program: None,
            u_projection: None,
            u_viewport: None,
            u_focal: None,
            u_htan_fov: None,
            u_view: None,
            u_cam_pos: None,
            u_splat_scale: None,

            vertex_buffer: None,
            a_position: 0,

            texture: None,
            u_splat_texture: None,

            index_buffer: None,
            a_index: 0,
        }
    }


    pub fn init(
        &mut self,
        gl: &Context,
        error_flag: &Arc<AtomicBool>,
        error_msg: &Arc<Mutex<String>>,
        scene: &Arc<Scene>
    ) {
        let gsplat_program_id = create_glsl_program(
            gl,
            Self::VERT_SHADER,
            Self::FRAG_SHADER,
            error_flag,
            error_msg
        );
        self.program = Some(gsplat_program_id);
        log!("SplatGLSL::init(): self.program={:?}", self.program);

        unsafe {
            gl.use_program(self.program);
            {
                self.u_projection = gl.get_uniform_location(gsplat_program_id, "projection");
                log!("SplatGLSL::init(): self.u_projection={:?}", self.u_projection);
                self.u_viewport = gl.get_uniform_location(gsplat_program_id, "viewport");
                log!("SplatGLSL::init(): self.u_viewport={:?}", self.u_viewport);
                self.u_focal = gl.get_uniform_location(gsplat_program_id, "focal");
                log!("SplatGLSL::init(): self.u_focal={:?}", self.u_focal);
                self.u_view = gl.get_uniform_location(gsplat_program_id, "view");
                log!("SplatGLSL::init(): self.u_view={:?}", self.u_view);
                self.u_htan_fov = gl.get_uniform_location(gsplat_program_id, "htan_fov");
                log!("SplatGLSL::init(): self.u_htan_fov={:?}", self.u_htan_fov);
                self.u_cam_pos = gl.get_uniform_location(gsplat_program_id, "cam_pos");
                log!("SplatGLSL::init(): self.u_cam_pos={:?}", self.u_cam_pos);
                self.u_splat_scale = gl.get_uniform_location(gsplat_program_id, "splat_scale");
                log!("SplatGLSL::init(): self.u_splat_scale={:?}", self.u_splat_scale);

                let triangle_vertices = &mut [ // quad
                    -1_f32, -1.0,
                    1.0, -1.0,
                    1.0, 1.0,
                    -1.0, 1.0,
                ];
                triangle_vertices.iter_mut().for_each(|v| *v *= 2.0);
                self.vertex_buffer = Some(gl.create_buffer().unwrap());
                log!("SplatGLSL::init(): self.vertex_buffer={:?}", self.vertex_buffer);
                gl.bind_buffer(context::ARRAY_BUFFER, self.vertex_buffer);
                gl.buffer_data_u8_slice(context::ARRAY_BUFFER, transmute_slice::<_, u8>(triangle_vertices), context::STATIC_DRAW);
                self.a_position = gl.get_attrib_location(gsplat_program_id, "position").unwrap();
                log!("SplatGLSL::init(): self.a_position={:?}", self.a_position);
                gl.enable_vertex_attrib_array(self.a_position);
                gl.bind_buffer(context::ARRAY_BUFFER, self.vertex_buffer);
                gl.vertex_attrib_pointer_f32(self.a_position, 2, context::FLOAT, false, 0, 0);

                self.texture = Some(gl.create_texture().unwrap());
                log!("SplatGLSL::init(): self.texture={:?}", self.texture); // WebTextureKey(1v1)
                gl.bind_texture(context::TEXTURE_2D, self.texture);
                self.u_splat_texture = gl.get_uniform_location(gsplat_program_id, "u_splat_texture");
                log!("SplatGLSL::init(): self.u_splat_texture={:?}", self.u_splat_texture);
                gl.uniform_1_i32(self.u_splat_texture.as_ref(), 0); // associate the active texture unit with the uniform

                // index buffer for instanced rendering
                self.index_buffer = Some(gl.create_buffer().unwrap());
                log!("SplatGLSL::init(): self.index_buffer={:?}", self.index_buffer);
                //gl.bind_buffer(context::ARRAY_BUFFER, self.index_buffer);
                self.a_index = gl.get_attrib_location(gsplat_program_id, "index").unwrap();
                log!("SplatGLSL::init(): self.a_index={:?}", self.a_index);
                gl.enable_vertex_attrib_array(self.a_index);
                gl.bind_buffer(context::ARRAY_BUFFER, self.index_buffer);
                gl.vertex_attrib_pointer_i32(self.a_index, 1, context::INT, 0, 0);
                gl.vertex_attrib_divisor(self.a_index, 1);
            }
            gl.use_program(None);

            gl.bind_texture(context::TEXTURE_2D, self.texture);
            gl.tex_parameter_i32(context::TEXTURE_2D, context::TEXTURE_WRAP_S, context::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(context::TEXTURE_2D, context::TEXTURE_WRAP_T, context::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(context::TEXTURE_2D, context::TEXTURE_MIN_FILTER, context::NEAREST as i32);
            gl.tex_parameter_i32(context::TEXTURE_2D, context::TEXTURE_MAG_FILTER, context::NEAREST as i32);

            #[cfg(not(feature = "async_splat_stream"))]
            gl.tex_image_2d(
                context::TEXTURE_2D,
                0,
                context::RGBA32UI as i32,
                scene.tex_width as i32,
                scene.tex_height as i32,
                0,
                context::RGBA_INTEGER,
                context::UNSIGNED_INT,
                Some(transmute_slice::<_, u8>(scene.tex_data.as_slice()))
            );

            //gl.active_texture(context::TEXTURE0);
            //gl.bind_texture(context::TEXTURE_2D, self.texture);

            gl.bind_buffer(context::ARRAY_BUFFER, None);
            gl.bind_texture(context::TEXTURE_2D, None);
        }
    }


    pub fn render(
        &self,
        gl: &Context,
        projection_slice: &[f32],
        view_slice: &[f32],
        focal: &[f32],
        viewport: &[f32],
        htan_fov: &[f32],
        cam_pos: &[f32],
        splat_scale: f32,
        rx_depth: &mut BusReader<Vec<u32>>,
        splat_count: i32
    ) {
        unsafe {
            gl.use_program(self.program);
            {
                gl.disable(context::DEPTH_TEST);
                gl.disable(context::CULL_FACE);
                //gl.cull_face(context::FRONT);

                // FIXME
                gl.enable(context::BLEND);
                /*
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.blend_func(context::SRC_ALPHA, context::ONE_MINUS_SRC_ALPHA);
                //gl.blend_func(context::ONE_MINUS_SRC_ALPHA, context::SRC_ALPHA);
                */
                /*
                //gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.blend_func_separate(
                    context::ONE_MINUS_DST_ALPHA,
                    context::ONE,
                    context::ONE_MINUS_DST_ALPHA,
                    context::ONE,
                );
                gl.blend_equation_separate(context::FUNC_ADD, context::FUNC_ADD);
                */

                gl.uniform_matrix_4_f32_slice(self.u_projection.as_ref(), false, projection_slice);
                gl.uniform_matrix_4_f32_slice(self.u_view.as_ref(), false, view_slice);
                gl.uniform_1_i32(self.u_splat_texture.as_ref(), 0); // associate the active texture unit with the uniform
                gl.uniform_2_f32_slice(self.u_focal.as_ref(), focal);
                gl.uniform_2_f32_slice(self.u_viewport.as_ref(), viewport);
                gl.uniform_2_f32_slice(self.u_htan_fov.as_ref(), htan_fov);
                gl.uniform_3_f32_slice(self.u_cam_pos.as_ref(), cam_pos);
                gl.uniform_1_f32(self.u_splat_scale.as_ref(), splat_scale);

                gl.active_texture(context::TEXTURE0);
                gl.bind_texture(context::TEXTURE_2D, self.texture);

                gl.enable_vertex_attrib_array(self.a_position);
                gl.bind_buffer(context::ARRAY_BUFFER, self.vertex_buffer);
                gl.vertex_attrib_pointer_f32(self.a_position, 2, context::FLOAT, false, 0, 0);

                gl.enable_vertex_attrib_array(self.a_index);
                gl.bind_buffer(context::ARRAY_BUFFER, self.index_buffer);
                //////////////////////////////////
                // non-blocking (i.e., no atomic.wait)
                if let Ok(depth_index) = rx_depth.try_recv() {
                    gl.buffer_data_u8_slice(
                        context::ARRAY_BUFFER,
                        transmute_slice::<_, u8>(depth_index.as_slice()),
                        context::DYNAMIC_DRAW
                    );
                }
                //////////////////////////////////
                gl.vertex_attrib_pointer_i32(self.a_index, 1, context::INT, 0, 0);
                gl.vertex_attrib_divisor(self.a_index, 1);

                gl.draw_arrays_instanced(
                    context::TRIANGLE_FAN,
                    0,
                    4,
                    splat_count
                );
            }
            gl.use_program(None);
            gl.bind_buffer(context::ARRAY_BUFFER, None);
            gl.bind_texture(context::TEXTURE_2D, None);
        }
    }
}


struct QuadGLSL {
    // render to texture
    pub(crate) framebuffer: Option<context::Framebuffer>,
    texture: Option<context::WebTextureKey>,

    // textured quad
    program: Option<context::Program>,
    vao: Option<context::VertexArray>,
    vbo: Option<context::WebBufferKey>,
    a_position: u32,
    u_screen_texture: Option<context::UniformLocation>,
}
impl QuadGLSL {
    const VERT_SHADER: &'static str = include_str!("quad.vert");
    const FRAG_SHADER: &'static str = include_str!("quad.frag");
    const VERTICES: &'static [f32; 18] = &[
        // XYZ
        -1.0,  1.0, 0.0,
        -1.0, -1.0, 0.0,
         1.0, -1.0, 0.0,

        -1.0,  1.0, 0.0,
         1.0, -1.0, 0.0,
         1.0,  1.0, 0.0,
    ];


    pub fn new() -> Self {
        Self {
            framebuffer: None,
            texture: None,

            program: None,
            vao: None,
            vbo: None,
            a_position: 0,
            u_screen_texture: None,
        }
    }


    pub fn init(
        &mut self,
        gl: &Context,
        error_flag: &Arc<AtomicBool>,
        error_msg: &Arc<Mutex<String>>,
        width: i32,
        height: i32
    ) {
        let quad_program_id = create_glsl_program(
            gl,
            Self::VERT_SHADER,
            Self::FRAG_SHADER,
            error_flag,
            error_msg
        );
        self.program = Some(quad_program_id);
        log!("QuadGLSL::init(): self.program={:?}", self.program);

        unsafe {
            self.framebuffer = Some(gl.create_framebuffer().unwrap());
            log!("QuadGLSL::init(): self.framebuffer={:?}", self.framebuffer);
            gl.bind_framebuffer(context::FRAMEBUFFER, self.framebuffer);
            {
                self.texture = Some(gl.create_texture().unwrap());
                log!("QuadGLSL::init(): self.texture={:?}", self.texture);
                gl.bind_texture(context::TEXTURE_2D, self.texture);
                gl.tex_image_2d(
                    context::TEXTURE_2D,
                    0,
                    context::RGB as i32,
                    width,
                    height,
                    0,
                    context::RGB,
                    context::UNSIGNED_BYTE,
                    None
                );
                gl.tex_parameter_i32(context::TEXTURE_2D, context::TEXTURE_MIN_FILTER, context::LINEAR as i32);
                gl.tex_parameter_i32(context::TEXTURE_2D, context::TEXTURE_MAG_FILTER, context::LINEAR as i32);

                gl.framebuffer_texture_2d(
                    context::FRAMEBUFFER,
                    context::COLOR_ATTACHMENT0,
                    context::TEXTURE_2D,
                    self.texture,
                    0
                );

                let status = gl.check_framebuffer_status(context::FRAMEBUFFER);
                if status != context::FRAMEBUFFER_COMPLETE {
                    set_error_for_egui(
                        error_flag, error_msg,
                        format!("ERROR: gl.check_framebuffer_status(): {}", status)
                    );
                }
            }
            gl.bind_framebuffer(context::FRAMEBUFFER, None);
            gl.bind_texture(context::TEXTURE_2D, None);

            gl.use_program(self.program);
            {
                self.vao = Some(gl.create_vertex_array().unwrap());
                log!("QuadGLSL::init(): self.vao={:?}", self.vao);
                gl.bind_vertex_array(self.vao);

                self.vbo = Some(gl.create_buffer().unwrap());
                log!("QuadGLSL::init(): self.vbo={:?}", self.vbo);
                gl.bind_buffer(context::ARRAY_BUFFER, self.vbo);
                gl.buffer_data_u8_slice(context::ARRAY_BUFFER, transmute_slice::<_, u8>(Self::VERTICES), context::STATIC_DRAW);

                self.a_position = gl.get_attrib_location(quad_program_id, "position").unwrap();
                log!("QuadGLSL::init(): self.a_position={:?}", self.a_position);
                gl.enable_vertex_attrib_array(self.a_position);
                gl.vertex_attrib_pointer_f32(
                    self.a_position,
                    3,
                    context::FLOAT,
                    false,
                    3*std::mem::size_of::<f32>() as i32,
                    0
                );

                self.u_screen_texture = gl.get_uniform_location(quad_program_id, "u_screen_texture");
                log!("QuadGLSL::init(): self.u_screen_texture={:?}", self.u_screen_texture);
                gl.uniform_1_i32(self.u_screen_texture.as_ref(), 0); // associate the active texture unit with the uniform
            }
            gl.use_program(None);
            gl.bind_vertex_array(None);
            gl.bind_buffer(context::ARRAY_BUFFER, None);
        }
    }


    pub fn render(
        &self,
        gl: &Context,
    ) {
        unsafe {
            gl.use_program(self.program);
            {
                gl.uniform_1_i32(self.u_screen_texture.as_ref(), 0);

                gl.active_texture(context::TEXTURE0);
                gl.bind_texture(context::TEXTURE_2D, self.texture);

                gl.bind_vertex_array(self.vao);
                gl.draw_arrays(context::TRIANGLES, 0, 6);
            }
            gl.use_program(None);
        }
    }
}

#[allow(unused_mut)]
pub async fn main() {
    let error_flag = Arc::new(AtomicBool::new(false));
    let error_msg = Arc::new(Mutex::new(String::new()));

    let cpu_cores = cpu_cores() as usize;
    log!("main(): cpu_cores={}", cpu_cores);

    let canvas_w = get_canvas_width();
    let canvas_h = get_canvas_height();
    log!("main(): canvas size: {}x{}", canvas_w, canvas_h);

    let window = Window::new(WindowSettings {
        title: "Gauzilla: 3D Gaussian Splatting in WASM + WebGL".to_string(),
        max_size: Some((canvas_w, canvas_h)),
        ..Default::default()
    })
    .unwrap();

    let gl = window.gl();
    log!("main(): OpenGL version: {:?}", gl.version());
    let glsl_ver = unsafe { gl.get_parameter_string(context::SHADING_LANGUAGE_VERSION) };
    log!("main(): GLSL version: {}", glsl_ver);

    let fovy = degrees(45.0);

    let mut camera = Camera::new_perspective(
        window.viewport(),
        get_position(),
        get_target(),
        get_up(),
        fovy,
        0.1,//0.2,
        10.0,//200.0,
    );
    let mut orbit_control = OrbitControl2::new(*camera.target(), 1.0, 100.0);
    let mut fly_control = FlyControl::new(0.005);
    let mut egui_control = TdCameraControl::Orbit;

    // lock-free bus for streamed scene buffer (single-send, multi-consumer)
    let mut bus_buffer = Bus::<Vec::<u8>>::new(1);
    let rx_buffer_threaded = bus_buffer.add_rx();
    let mut rx_buffer = bus_buffer.add_rx();
    let bus_buffer_rc =  Rc::new(RefCell::new(bus_buffer));

    // lock-free bus for scene buffer (single-send, single-consumer)
    let mut bus_progress = Bus::<f64>::new(10);
    let mut rx_progress = bus_progress.add_rx();
    let bus_progress_rc =  Rc::new(RefCell::new(bus_progress));

    let mut url = get_url_param();
    if url.is_empty() {
        url = "https://huggingface.co/datasets/satyoshi/gauzilla-data/resolve/main/book_store.splat".to_string();
    }
    log!("main(): url={}", url);

    #[cfg(feature = "async_splat_stream")]
    let worker_handle = stream_splat_in_worker(bus_buffer_rc, bus_progress_rc, url);
    #[cfg(feature = "async_splat_stream")]
    //let mut scene = Scene::new();
    let mut scene = Arc::new(Scene::new());
    #[cfg(not(feature = "async_splat_stream"))]
    let scene = Arc::new(load_scene().await);

    let mut splat_glsl = SplatGLSL::new();
    splat_glsl.init(&gl, &error_flag, &error_msg, &scene);

    let mut quad_glsl = QuadGLSL::new();
    quad_glsl.init(&gl, &error_flag, &error_msg, canvas_w as i32, canvas_h as i32);

    // TODO: implement resize() for change in window size

    // lock-free bus for depth_index
    let mut bus_depth_threaded = Bus::<Vec<u32>>::new(10);
    let mut rx_depth = bus_depth_threaded.add_rx();

    // lock-free bus for view_proj_slice
    let mut bus_vp = Bus::<Mat4>::new(10);
    let rx_vp_threaded: BusReader<Matrix4<f32>> = bus_vp.add_rx();

    // lock-free bus for sort_time
    let mut bus_time_threaded = Bus::<f64>::new(10);
    let mut rx_time = bus_time_threaded.add_rx();

    let thread_handle = launch_sorter_thread(
        scene.clone(),
        rx_buffer_threaded,
        rx_vp_threaded,
        bus_depth_threaded,
        cpu_cores,
        bus_time_threaded,
    );

    /////////////////////////////////////////////////////////////////////////////////

    let mut gui = three_d::GUI::new(&gl);
    let mut pointer_over_gui = false;
    let mut splat_scale = 1_f32;
    let mut cam_roll = 0_f32;
    let mut prev_cam_roll = 0_f32;
    let mut flip_y = true;
    let mut frame_prev = get_time_milliseconds();
    let mut fps_ma = IncrementalMA::new(100);
    let mut sort_time = 0_f64;
    let mut sort_time_ma = IncrementalMA::new(100);
    let mut send_view_proj: bool = true;
    let mut progress = 0_f64;
    let mut s_temp = Scene::new();

    #[cfg(not(feature = "async_splat_stream"))]
    let done_streaming = true;
    #[cfg(feature = "async_splat_stream")]
    let mut done_streaming = false;

    window.render_loop(move |mut frame_input| {
        let error_flag = Arc::clone(&error_flag);
        let error_msg = Arc::clone(&error_msg);

        let now =  get_time_milliseconds();
        let fps =  1000.0 / (now - frame_prev);
        frame_prev = now;
        let fps = fps_ma.add(fps);

        if !error_flag.load(Ordering::Relaxed) {
            /////////////////////////////////////////////////////////////////////////////////////
            // receive sort_time from the second thread
            if let Ok(f) = rx_time.try_recv() {
                sort_time = sort_time_ma.add(f);
            }

            #[cfg(feature = "async_splat_stream")]
            if !done_streaming {
                // receive progress from async JS worker callback
                if let Ok(pct) = rx_progress.try_recv() {
                    progress = pct;
                }

                // receive splat binary buffer from async JS worker callback
                if let Ok(buffer) = rx_buffer.try_recv() {
                    let mut s = Scene::new();
                    s.buffer = buffer;
                    s.splat_count = s.buffer.len() / 32; // 32bytes per splat
                    s.generate_texture();
                    scene = Arc::new(s);

                    unsafe {
                        gl.bind_texture(context::TEXTURE_2D, splat_glsl.texture);
                        gl.tex_image_2d(
                            context::TEXTURE_2D,
                            0,
                            context::RGBA32UI as i32,
                            scene.tex_width as i32,
                            scene.tex_height as i32,
                            0,
                            context::RGBA_INTEGER,
                            context::UNSIGNED_INT,
                            Some(transmute_slice::<_, u8>(scene.tex_data.as_slice()))
                        );
                    }

                    done_streaming = true;
                    send_view_proj = true;
                }

                /*
                // receive splat chunk from async JS worker callback
                if let Ok(chunk) = rx_buffer.try_recv() {
                    scene.buffer.extend(chunk);
                    scene.splat_count = scene.buffer.len() / 32; // 32bytes per splat
                }
                // FIXME
                log!("main(): progress={}", progress);
                if progress >= 1.0 {
                    log!("main(): done streaming");
                    worker_handle.terminate(); // no longer need to receive buffer

                    scene.generate_texture();
                    unsafe {
                        gl.bind_texture(context::TEXTURE_2D, splat_texture);
                        gl.tex_image_2d(
                            context::TEXTURE_2D,
                            0,
                            context::RGBA32UI as i32,
                            scene.tex_width as i32,
                            scene.tex_height as i32,
                            0,
                            context::RGBA_INTEGER,
                            context::UNSIGNED_INT,
                            Some(transmute_slice::<_, u8>(scene.tex_data.as_slice()))
                        );
                    }

                    done_streaming = true;
                    send_view_proj = true;
                }
                */
            }

            /////////////////////////////////////////////////////////////////////////////////////

            camera.set_viewport(frame_input.viewport);

            for event in frame_input.events.iter() {
                send_view_proj = true;

                /*
                if let Event::MousePress {
                    button,
                    position,
                    modifiers,
                    ..
                } = event
                {
                    if *button == MouseButton::Right && !modifiers.ctrl {
                        log!("right mouse button pressed at {:?}", position);
                    }
                }
                */

                /*
                if let Event::MouseMotion {
                    delta,
                    button,
                    handled,
                    ..
                } = event {
                }
                */
            }

            if !pointer_over_gui {
                match egui_control {
                    TdCameraControl::Orbit => {
                        orbit_control.handle_events(&mut camera, &mut frame_input.events);
                    },
                    TdCameraControl::Fly => {
                        fly_control.handle_events(&mut camera, &mut frame_input.events);
                    },
                }
            }

            if flip_y {
                //camera.mirror_in_xz_plane(); // FIXME
                camera.roll(degrees(180.0));
                flip_y = false;
            }
            if !are_floats_equal(cam_roll, prev_cam_roll, 0.00001) {
                camera.roll(degrees(-prev_cam_roll));
                camera.roll(degrees(cam_roll));
                prev_cam_roll = cam_roll;
            }
        }

        let view_matrix: &Mat4 = camera.view();
        let view_slice = &[
            view_matrix[0][0], view_matrix[0][1], view_matrix[0][2], view_matrix[0][3],
            view_matrix[1][0], view_matrix[1][1], view_matrix[1][2], view_matrix[1][3],
            view_matrix[2][0], view_matrix[2][1], view_matrix[2][2], view_matrix[2][3],
            view_matrix[3][0], view_matrix[3][1], view_matrix[3][2], view_matrix[3][3]
        ];
        let projection_matrix: &Mat4 = camera.projection();
        let projection_slice = &[
            projection_matrix[0][0], projection_matrix[0][1], projection_matrix[0][2], projection_matrix[0][3],
            projection_matrix[1][0], projection_matrix[1][1], projection_matrix[1][2], projection_matrix[1][3],
            projection_matrix[2][0], projection_matrix[2][1], projection_matrix[2][2], projection_matrix[2][3],
            projection_matrix[3][0], projection_matrix[3][1], projection_matrix[3][2], projection_matrix[3][3]
        ];
        let w = camera.viewport().width as f32;
        let h = camera.viewport().height as f32;
        let cam_pos = camera.position();
        let fx = 0.5*projection_matrix[0][0]*w;
        let fy = -0.5*projection_matrix[1][1]*h;
        let htany = (fovy / 2.0).tan() as f32;
        let htanx = (htany/h)*w;
        //let focal = h / (2.0 * htany); // == fx == -fy

        gui.update(
            &mut frame_input.events,
            frame_input.accumulated_time,
            frame_input.viewport,
            frame_input.device_pixel_ratio,
            |gui_context| {
                pointer_over_gui = gui_context.is_using_pointer();//.is_pointer_over_area();

                if error_flag.load(Ordering::Relaxed) {
                    egui::Window::new("Error")
                        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                        .show(gui_context, |ui| {
                            {
                                let mutex = error_msg.lock().unwrap();
                                ui.colored_label(egui::Color32::RED, &(*mutex))
                            }
                            /*
                            if ui.button("Ok").clicked() {
                                error_flag.store(false, Ordering::Relaxed);
                            }
                            */
                        });
                } else {
                    if !done_streaming {
                        egui::Window::new("Loading...")
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .show(gui_context, |ui| {
                                let progress_bar = egui::ProgressBar::new(progress as f32)
                                    .show_percentage()
                                    .animate(false);
                                ui.add(progress_bar);

                            });
                    } else {
                        egui::Window::new("Gauzilla")
                            //.vscroll(true)
                            .show(gui_context, |ui| {
                            /*
                            // TODO: open a PLY file as bytes and process it
                            if ui.button("Open PLY file").clicked() {
                                let task = rfd::AsyncFileDialog::new()
                                    .add_filter("ply", &["ply"])
                                    .pick_file();
                                execute_future(async move {
                                    let file = task.await;
                                    if let Some(f) = file {
                                        let bytes = f.read().await;
                                        match Scene::parse_file_header(bytes) {
                                            Ok((file_header_size, splat_count, mut cursor)) => {

                                            },
                                            Err(s) => set_error_for_egui(
                                                &error_flag, &error_msg, String::from("ERROR: could not open the selected file.\
                                                Choose a correctly formatted PLY file for 3D Gaussian Splatting.")
                                            ),
                                        }
                                    }
                                });
                                ui.close_menu();
                            }
                            */

                            egui::Grid::new("my_grid")
                                .num_columns(2)
                                .spacing([40.0, 4.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    ui.add(egui::Label::new("FPS"));
                                    ui.label(format!("{:.2}", fps));
                                    ui.end_row();

                                    ui.add(egui::Label::new("CPU Sort Time (ms)"));
                                    ui.label(format!("{:.2}", sort_time));
                                    ui.end_row();

                                    ui.add(egui::Label::new("CPU Cores"));
                                    ui.label(format!("{}", cpu_cores));
                                    ui.end_row();

                                    ui.add(egui::Label::new("GL Version"));
                                    ui.label(format!("{:?}", gl.version()));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Splat Count"));
                                    ui.label(format!("{}", scene.splat_count.to_formatted_string(&Locale::en)));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Splat Scale"));
                                    ui.add(egui::Slider::new(&mut splat_scale, 0.1..=1.0));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Invert Y"));
                                    ui.checkbox(&mut flip_y, "");
                                    ui.end_row();

                                    ui.add(egui::Label::new("Window Size"));
                                    ui.label(format!("{}x{}", w, h));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Focal"));
                                    ui.label(format!("({:.2}, {:.2})", fx, fy));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Htan FOV"));
                                    ui.label(format!("({:.2}, {:.2})", htanx, htany));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Camera Position"));
                                    ui.label(format!("({:.2}, {:.2}, {:.2})", cam_pos.x, cam_pos.y, cam_pos.z));
                                    ui.end_row();

                                    ui.add(egui::Label::new("Camera Control"));
                                    ui.horizontal(|ui| {
                                        ui.radio_value(&mut egui_control, TdCameraControl::Orbit, "Orbit");
                                        ui.radio_value(&mut egui_control, TdCameraControl::Fly, "Fly");
                                    });
                                    ui.end_row();

                                    ui.add(egui::Label::new("Camera Roll"));
                                    ui.add(egui::Slider::new(&mut cam_roll, -180.0..=180.0).suffix("°"));
                                    ui.end_row();

                                    ui.add(egui::Label::new("GitHub"));
                                    use egui::special_emojis::GITHUB;
                                    ui.hyperlink_to(
                                        format!("{GITHUB} BladeTransformerLLC/gauzilla"),
                                        "https://github.com/BladeTransformerLLC/gauzilla",
                                    );
                                    ui.end_row();
                                });
                        });
                    }
                }
            },
        );

        if !error_flag.load(Ordering::Relaxed) {
            // send view_proj to thread only when it's changed by user input
            if done_streaming && send_view_proj  {
                let view_proj = projection_matrix * view_matrix;
                //////////////////////////////////
                // non-blocking (i.e., no atomic.wait)
                let _ = bus_vp.try_broadcast(view_proj);
                //////////////////////////////////
                send_view_proj = false;
            }

            unsafe {
                // render to texture
                gl.bind_framebuffer(context::FRAMEBUFFER, quad_glsl.framebuffer);
                {
                    gl.viewport(0, 0, w as i32, h as i32);
                    gl.clear(context::COLOR_BUFFER_BIT);

                    splat_glsl.render(
                        &gl,
                        projection_slice,
                        view_slice,
                        &[fx.abs(), fy.abs()],
                        &[w, h],
                        &[htanx, htany],
                        &[cam_pos.x, cam_pos.y, cam_pos.z],
                        splat_scale,
                        &mut rx_depth,
                        scene.splat_count as i32
                    );
                }
                gl.bind_framebuffer(context::FRAMEBUFFER, None);

                { // render the textured quad
                    gl.viewport(0, 0, w as i32, h as i32);
                    gl.clear(context::COLOR_BUFFER_BIT);

                    quad_glsl.render(&gl);
                }

                gui.render();
                gl.flush();
            }
        } else {
            gui.render();
        }

        // Returns default frame output to end the frame
        FrameOutput::default()
    });

    // thread exit is not implemented in rustwasm yet
    // https://rustwasm.github.io/2018/10/24/multithreading-rust-and-wasm.html
    let _ = thread_handle.join();

}


