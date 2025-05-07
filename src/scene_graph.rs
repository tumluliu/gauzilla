use serde::{Deserialize, Serialize};
use std::sync::{atomic::AtomicBool, Arc, Mutex};
use three_d::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneGraphNode {
    pub name: String,
    pub position: [f32; 3],
    pub children: Vec<SceneGraphNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneGraph {
    pub root: SceneGraphNode,
}

pub struct SceneGraphRenderer {
    program: Option<context::Program>,
    u_projection: Option<context::UniformLocation>,
    u_view: Option<context::UniformLocation>,
    u_model: Option<context::UniformLocation>,
    u_color: Option<context::UniformLocation>,

    // For nodes (spheres)
    sphere_vertices: Option<context::WebBufferKey>,
    sphere_indices: Option<context::WebBufferKey>,
    a_position: u32,
    num_sphere_indices: usize,

    // For edges (lines)
    line_vertices: Option<context::WebBufferKey>,
    a_line_position: u32,

    // Global scale for scene graph positions
    pub global_scale: f32,
}

impl SceneGraphRenderer {
    const VERT_SHADER: &'static str = r#"#version 300 es
        precision highp float;
        
        in vec3 position;
        
        uniform mat4 projection;
        uniform mat4 view;
        uniform mat4 model;
        
        void main() {
            gl_Position = projection * view * model * vec4(position, 1.0);
        }
    "#;

    const FRAG_SHADER: &'static str = r#"#version 300 es
        precision highp float;
        
        uniform vec4 color;
        out vec4 fragColor;
        
        void main() {
            fragColor = color;
        }
    "#;

    pub fn new() -> Self {
        Self {
            program: None,
            u_projection: None,
            u_view: None,
            u_model: None,
            u_color: None,
            sphere_vertices: None,
            sphere_indices: None,
            a_position: 0,
            num_sphere_indices: 0,
            line_vertices: None,
            a_line_position: 0,
            global_scale: 0.1,
        }
    }

    pub fn init(
        &mut self,
        gl: &Context,
        error_flag: &Arc<AtomicBool>,
        error_msg: &Arc<Mutex<String>>,
    ) {
        let program_id = create_glsl_program(
            gl,
            Self::VERT_SHADER,
            Self::FRAG_SHADER,
            error_flag,
            error_msg,
        );
        self.program = Some(program_id);

        unsafe {
            gl.use_program(self.program);
            {
                self.u_projection = gl.get_uniform_location(program_id, "projection");
                self.u_view = gl.get_uniform_location(program_id, "view");
                self.u_model = gl.get_uniform_location(program_id, "model");
                self.u_color = gl.get_uniform_location(program_id, "color");

                // Create sphere geometry
                let (vertices, indices) = self.generate_sphere(16, 16);
                self.num_sphere_indices = indices.len();
                self.sphere_vertices = Some(gl.create_buffer().unwrap());
                gl.bind_buffer(context::ARRAY_BUFFER, self.sphere_vertices);
                gl.buffer_data_u8_slice(
                    context::ARRAY_BUFFER,
                    transmute_slice::<_, u8>(&vertices),
                    context::STATIC_DRAW,
                );

                self.sphere_indices = Some(gl.create_buffer().unwrap());
                gl.bind_buffer(context::ELEMENT_ARRAY_BUFFER, self.sphere_indices);
                gl.buffer_data_u8_slice(
                    context::ELEMENT_ARRAY_BUFFER,
                    transmute_slice::<_, u8>(&indices),
                    context::STATIC_DRAW,
                );

                self.a_position = gl.get_attrib_location(program_id, "position").unwrap();
                gl.enable_vertex_attrib_array(self.a_position);
                gl.vertex_attrib_pointer_f32(self.a_position, 3, context::FLOAT, false, 0, 0);

                // Create line geometry
                self.line_vertices = Some(gl.create_buffer().unwrap());
                gl.bind_buffer(context::ARRAY_BUFFER, self.line_vertices);
                self.a_line_position = gl.get_attrib_location(program_id, "position").unwrap();
                gl.enable_vertex_attrib_array(self.a_line_position);
                gl.vertex_attrib_pointer_f32(self.a_line_position, 3, context::FLOAT, false, 0, 0);
            }
            gl.use_program(None);
        }
    }

    fn generate_sphere(&self, lat_segments: usize, long_segments: usize) -> (Vec<f32>, Vec<u32>) {
        let mut vertices = Vec::with_capacity((lat_segments + 1) * (long_segments + 1) * 3);
        let mut indices = Vec::with_capacity(lat_segments * long_segments * 6);

        for lat in 0..=lat_segments {
            let theta = lat as f32 * std::f32::consts::PI / lat_segments as f32;
            let sin_theta = theta.sin();
            let cos_theta = theta.cos();

            for long in 0..=long_segments {
                let phi = long as f32 * 2.0 * std::f32::consts::PI / long_segments as f32;
                let sin_phi = phi.sin();
                let cos_phi = phi.cos();

                let x = sin_theta * cos_phi;
                let y = cos_theta;
                let z = sin_theta * sin_phi;

                vertices.extend_from_slice(&[x, y, z]);
            }
        }

        for lat in 0..lat_segments {
            for long in 0..long_segments {
                let first = (lat * (long_segments + 1) + long) as u32;
                let second = first + 1;
                let third = ((lat + 1) * (long_segments + 1) + long) as u32;
                let fourth = third + 1;

                indices.extend_from_slice(&[first, second, third]);
                indices.extend_from_slice(&[second, fourth, third]);
            }
        }

        (vertices, indices)
    }

    pub fn render(&self, gl: &Context, projection: &[f32], view: &[f32], scene_graph: &SceneGraph) {
        unsafe {
            gl.use_program(self.program);
            {
                gl.uniform_matrix_4_f32_slice(self.u_projection.as_ref(), false, projection);
                gl.uniform_matrix_4_f32_slice(self.u_view.as_ref(), false, view);

                // Render nodes and edges recursively
                self.render_node(gl, &scene_graph.root, &Mat4::identity());
            }
            gl.use_program(None);
        }
    }

    fn render_node(&self, gl: &Context, node: &SceneGraphNode, parent_transform: &Mat4) {
        unsafe {
            // Local translation (apply global scale)
            let local_translation = Mat4::from_translation(vec3(
                node.position[0] * self.global_scale,
                node.position[1] * self.global_scale,
                node.position[2] * self.global_scale,
            ));
            // Accumulated world transform
            let world_transform = *parent_transform * local_translation;

            // Draw node as sphere
            let scale = if node.children.is_empty() { 0.1 } else { 0.2 };
            let scale_matrix = Mat4::from_scale(scale);
            let final_model = world_transform * scale_matrix;
            let final_model_array = [
                final_model.x.x,
                final_model.y.x,
                final_model.z.x,
                final_model.w.x,
                final_model.x.y,
                final_model.y.y,
                final_model.z.y,
                final_model.w.y,
                final_model.x.z,
                final_model.y.z,
                final_model.z.z,
                final_model.w.z,
                final_model.x.w,
                final_model.y.w,
                final_model.z.w,
                final_model.w.w,
            ];
            gl.uniform_matrix_4_f32_slice(self.u_model.as_ref(), false, &final_model_array);
            gl.uniform_4_f32(self.u_color.as_ref(), 1.0, 0.0, 0.0, 1.0); // Red for nodes

            gl.bind_buffer(context::ARRAY_BUFFER, self.sphere_vertices);
            gl.bind_buffer(context::ELEMENT_ARRAY_BUFFER, self.sphere_indices);
            gl.enable_vertex_attrib_array(self.a_position);
            gl.vertex_attrib_pointer_f32(self.a_position, 3, context::FLOAT, false, 0, 0);
            gl.draw_elements(
                context::TRIANGLES,
                self.num_sphere_indices as i32,
                context::UNSIGNED_INT,
                0,
            );

            // Compute this node's world position
            let p = world_transform * vec4(0.0, 0.0, 0.0, 1.0);
            let this_world_pos = vec3(p.x, p.y, p.z);

            // Draw edges and recurse for children
            for child in &node.children {
                // Compute child's world transform
                let child_local_translation = Mat4::from_translation(vec3(
                    child.position[0] * self.global_scale,
                    child.position[1] * self.global_scale,
                    child.position[2] * self.global_scale,
                ));
                let child_world_transform = world_transform * child_local_translation;
                let cp = child_world_transform * vec4(0.0, 0.0, 0.0, 1.0);
                let child_world_pos = vec3(cp.x, cp.y, cp.z);

                // Draw edge
                let line_vertices = [
                    this_world_pos.x,
                    this_world_pos.y,
                    this_world_pos.z,
                    child_world_pos.x,
                    child_world_pos.y,
                    child_world_pos.z,
                ];
                gl.bind_buffer(context::ARRAY_BUFFER, self.line_vertices);
                gl.buffer_data_u8_slice(
                    context::ARRAY_BUFFER,
                    transmute_slice::<_, u8>(&line_vertices),
                    context::DYNAMIC_DRAW,
                );
                gl.enable_vertex_attrib_array(self.a_line_position);
                gl.vertex_attrib_pointer_f32(self.a_line_position, 3, context::FLOAT, false, 0, 0);
                gl.uniform_4_f32(self.u_color.as_ref(), 0.0, 1.0, 0.0, 1.0); // Green for edges
                gl.draw_arrays(context::LINES, 0, 2);

                // Recurse
                self.render_node(gl, child, &world_transform);
            }
        }
    }
}

// Helper function to create GLSL program
fn create_glsl_program(
    gl: &Context,
    vs_source: &str,
    fs_source: &str,
    error_flag: &Arc<AtomicBool>,
    error_msg: &Arc<Mutex<String>>,
) -> context::Program {
    unsafe {
        let vert_shader = gl
            .create_shader(context::VERTEX_SHADER)
            .expect("Failed creating vertex shader");
        let frag_shader = gl
            .create_shader(context::FRAGMENT_SHADER)
            .expect("Failed creating fragment shader");

        gl.shader_source(vert_shader, vs_source);
        gl.shader_source(frag_shader, fs_source);
        gl.compile_shader(vert_shader);
        gl.compile_shader(frag_shader);

        let id = gl.create_program().expect("Failed creating program");

        gl.attach_shader(id, vert_shader);
        gl.attach_shader(id, frag_shader);
        gl.link_program(id);

        if !gl.get_program_link_status(id) {
            let log = gl.get_shader_info_log(vert_shader);
            if !log.is_empty() {
                let mut msg = error_msg.lock().unwrap();
                *msg = format!("ERROR: gl.get_program_link_status(): {}", log);
                error_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let log = gl.get_shader_info_log(frag_shader);
            if !log.is_empty() {
                let mut msg = error_msg.lock().unwrap();
                *msg = format!("ERROR: gl.get_program_link_status(): {}", log);
                error_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let log = gl.get_program_info_log(id);
            if !log.is_empty() {
                let mut msg = error_msg.lock().unwrap();
                *msg = format!("ERROR: gl.get_program_link_status(): {}", log);
                error_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        } else {
            gl.detach_shader(id, vert_shader);
            gl.detach_shader(id, frag_shader);
            gl.delete_shader(vert_shader);
            gl.delete_shader(frag_shader);
        }

        id
    }
}

// Helper function to transmute slice
fn transmute_slice<T, U>(slice: &[T]) -> &[U] {
    unsafe {
        std::slice::from_raw_parts(
            slice.as_ptr() as *const U,
            slice.len() * std::mem::size_of::<T>() / std::mem::size_of::<U>(),
        )
    }
}
