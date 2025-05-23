use std::{
    io::{BufRead, Cursor, BufReader, Read, Seek, SeekFrom},
    cmp::Ordering,
    sync::{Arc, Mutex},
};
use three_d::prelude::*;
use bus::Bus;
//use wasm_thread as thread;

use crate::log; // macro import
use crate::utils::*;
use crate::spz::{Spz, load_spz};


const MAX_HEADER_LINES: usize = 65;
const SH_C0: f32 = 0.28209479177387814;


#[derive(Clone)]
#[repr(C)]
struct SerializedSplat {
    position: [f32; 3], // center of the Gaussian ellipsoid
    n: [f32; 3], // unused normal
    color: [f32; 3*16], // RGB(3) + SH(45)
    alpha: f32, // opacity
    scale: [f32; 3], // scale of the Gaussian
    rotation: [f32; 4], // quaternion
} // 62*f32 (62*4=248bytes) in total
impl Default for SerializedSplat {
    fn default() -> Self {
        unsafe { std::mem::MaybeUninit::<SerializedSplat>::zeroed().assume_init() }
    }
}


#[derive(Clone)]
#[repr(C)]
pub struct SerializedSplat2 { // Scaniverse PLY format (no normals) / SPZ format
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 4],
    pub alpha: f32,
    pub color: [f32; 3*16],
} // 59*f32 (59*4=236bytes) in total
impl Default for SerializedSplat2 {
    fn default() -> Self {
        unsafe { std::mem::MaybeUninit::<SerializedSplat2>::zeroed().assume_init() }
    }
}


/// A point cloud of Gaussian splats
pub struct Scene {
    pub splat_count: usize,
    pub(crate) buffer: Vec<u8>,
    pub(crate) tex_data: Vec<u32>,
    pub(crate) tex_width: usize,
    pub(crate) tex_height: usize,
    prev_vp: Mutex<Vec<f32>>,
}
impl Scene {
    pub fn new() -> Self {
        Self {
            splat_count: 0,
            buffer: Vec::<u8>::new(),
            tex_data: Vec::<u32>::new(),
            tex_width: 0,
            tex_height: 0,
            prev_vp: Mutex::new(Vec::<f32>::new()),
        }
    }


    /// Parses the header of a PLY file
    /// Returns the header length in bytes, the number of splats in the file, and the file cursor
    pub fn parse_file_header(bytes: Vec<u8>) -> Result<(u16, usize, Cursor<Vec<u8>>), String> {
        let mut reader = BufReader::new(Cursor::new(bytes));
        let mut line = String::new();
        let mut splat_count: usize = 0;
        let mut success = false;
        let mut i = 0;

        loop {
            reader.read_line(&mut line).unwrap();
            if line == "end_header\n" {
                success = true;
                break;
            }
            if line.starts_with("element vertex ") {
                splat_count = line[15..line.len() - 1].parse().unwrap();
            }
            line.clear();

            i += 1;
            if i > MAX_HEADER_LINES {
                break;
            }
        }

        if !success {
            let error = "Scene::parse_file_header(): ERROR: the file is not correctly formatted.";
            log!("{}, i={}", error, i);
            return Err(error.to_string());
        }

        let file_header_size = reader.stream_position().unwrap() as u16;
        let cursor = reader.into_inner();
        log!(
            "Scene::parse_file_header(): i={}, file_header_size={}, splat_count={}",
            i,
            file_header_size,
            splat_count
        );

        Ok((file_header_size, splat_count, cursor))
    }


    /// Loads an entire PLY file into WASM memory
    pub fn load(&mut self, cursor: &mut Cursor<Vec<u8>>, file_header_size: u16) {
        let mut serialized_splats = vec![SerializedSplat::default(); self.splat_count];
        cursor.seek(SeekFrom::Start(file_header_size as u64)).unwrap();
        cursor.read_exact(transmute_slice_mut::<_, u8>(serialized_splats.as_mut_slice())).unwrap();

        // calculate importance of each splat
        let mut size_list = vec![0_f32; self.splat_count];
        let mut size_index = vec![0_u32; self.splat_count];
        for i in 0..self.splat_count {
            let s = &serialized_splats[i];
            size_index[i] = i as u32;
            let size = s.scale[0].exp()*s.scale[1].exp()*s.scale[2].exp();
            let opacity = 1.0 / (1.0 + (-s.alpha).exp());
            size_list[i] = (size as f32)*opacity;
        }

        // sort the indices of splats based on size_list in descending order
        size_index.sort_by(
            |&a, &b| size_list[b as usize]
                .partial_cmp(&size_list[a as usize])
                .unwrap_or(Ordering::Equal)
        );
        log!(
            "Scene::load(): size_list[0]={}, size_list[-1]={}",
            size_list[size_index[0] as usize],
            size_list[size_index[size_index.len()-1] as usize]
        );

        // construct a new binary buffer where each row corresponds to a splat in the sorted order.
        // XYZ - position (f32)
        // XYZ - scale (f32)
        // RGBA - color (u8)
        // IJKL - quaternion (u8)
        let row_length = 3*4 + 3*4 + 4 + 4; // 32bytes
        let mut buffer = vec![0_u8; row_length*self.splat_count];
        for i in 0..self.splat_count {
            let row = size_index[i] as usize;
            let s = &serialized_splats[row];

            let mut start = i*row_length;
            let mut end = start + 3*4;
            { // read 3x f32
                let position: &mut [f32] = transmute_slice_mut::<_, f32>(&mut buffer[start..end]);
                position[0] = s.position[0];
                position[1] = s.position[1];
                position[2] = s.position[2];
            }

            start = end;
            end = start + 3*4;
            { // read 3x f32
                let scales: &mut [f32] = transmute_slice_mut::<_, f32>(&mut buffer[start..end]);
                scales[0] = s.scale[0].exp();
                scales[1] = s.scale[1].exp();
                scales[2] = s.scale[2].exp();
            }

            // In Rust, float-to-integer casts saturate
            // (i.e., excess values are converted to T::MAX or T::MIN. NaN is converted to 0).

            start = end;
            end = start + 4;
            { // read 4x u8
                let rgba: &mut [u8] = transmute_slice_mut::<_, u8>(&mut buffer[start..end]);
                rgba[0] = ((0.5 + SH_C0*s.color[0]) * 255.0) as u8;
                rgba[1] = ((0.5 + SH_C0*s.color[1]) * 255.0) as u8;
                rgba[2] = ((0.5 + SH_C0*s.color[2]) * 255.0) as u8;
                rgba[3] = ((1.0 / (1.0 + (-s.alpha).exp()))*255.0) as u8; // opacity from sigmoid
            }

            start = end;
            end = start + 4;
            { // read 4x u8
                let rot: &mut [u8] = transmute_slice_mut::<_, u8>(&mut buffer[start..end]);
                let qlen = (s.rotation[0].powi(2) + s.rotation[1].powi(2) + s.rotation[2].powi(2) + s.rotation[3].powi(2)).sqrt();
                // [-1, 1] -> [0, 255]
                rot[0] = (((s.rotation[0]/qlen) + 1.0)*0.5 * 255.0) as u8;
                rot[1] = (((s.rotation[1]/qlen) + 1.0)*0.5 * 255.0) as u8;
                rot[2] = (((s.rotation[2]/qlen) + 1.0)*0.5 * 255.0) as u8;
                rot[3] = (((s.rotation[3]/qlen) + 1.0)*0.5 * 255.0) as u8;
            }
        }
        self.buffer = buffer;
    }


    /// Loads an entire PLY file (w/o normals) into WASM memory
    pub fn load_no_normal(&mut self, serialized_splats: Vec<SerializedSplat2>) { // TODO: remove code redundancy w/ load()
        // calculate importance of each splat
        let mut size_list = vec![0_f32; self.splat_count];
        let mut size_index = vec![0_u32; self.splat_count];
        for i in 0..self.splat_count {
            let s = &serialized_splats[i];
            size_index[i] = i as u32;
            let size = s.scale[0].exp()*s.scale[1].exp()*s.scale[2].exp();
            let opacity = 1.0 / (1.0 + (-s.alpha).exp());
            size_list[i] = (size as f32)*opacity;
        }

        // sort the indices of splats based on size_list in descending order
        size_index.sort_by(
            |&a, &b| size_list[b as usize]
                .partial_cmp(&size_list[a as usize])
                .unwrap_or(Ordering::Equal)
        );
        log!(
            "Scene::load_no_normal(): size_list[0]={}, size_list[-1]={}",
            size_list[size_index[0] as usize],
            size_list[size_index[size_index.len()-1] as usize]
        );

        // construct a new binary buffer where each row corresponds to a splat in the sorted order.
        // XYZ - position (f32)
        // XYZ - scale (f32)
        // RGBA - color (u8)
        // IJKL - quaternion (u8)
        let row_length = 3*4 + 3*4 + 4 + 4; // 32bytes
        let mut buffer = vec![0_u8; row_length*self.splat_count];
        for i in 0..self.splat_count {
            let row = size_index[i] as usize;
            let s = &serialized_splats[row];

            let mut start = i*row_length;
            let mut end = start + 3*4;
            { // read 3x f32
                let position: &mut [f32] = transmute_slice_mut::<_, f32>(&mut buffer[start..end]);
                position[0] = s.position[0];
                position[1] = s.position[1];
                position[2] = s.position[2];
            }

            start = end;
            end = start + 3*4;
            { // read 3x f32
                let scales: &mut [f32] = transmute_slice_mut::<_, f32>(&mut buffer[start..end]);
                scales[0] = s.scale[0].exp();
                scales[1] = s.scale[1].exp();
                scales[2] = s.scale[2].exp();
            }

            // In Rust, float-to-integer casts saturate
            // (i.e., excess values are converted to T::MAX or T::MIN. NaN is converted to 0).

            start = end;
            end = start + 4;
            { // read 4x u8
                let rgba: &mut [u8] = transmute_slice_mut::<_, u8>(&mut buffer[start..end]);
                rgba[0] = ((0.5 + SH_C0*s.color[0]) * 255.0) as u8;
                rgba[1] = ((0.5 + SH_C0*s.color[1]) * 255.0) as u8;
                rgba[2] = ((0.5 + SH_C0*s.color[2]) * 255.0) as u8;
                rgba[3] = ((1.0 / (1.0 + (-s.alpha).exp()))*255.0) as u8; // opacity from sigmoid
            }

            start = end;
            end = start + 4;
            { // read 4x u8
                let rot: &mut [u8] = transmute_slice_mut::<_, u8>(&mut buffer[start..end]);
                let qlen = (s.rotation[0].powi(2) + s.rotation[1].powi(2) + s.rotation[2].powi(2) + s.rotation[3].powi(2)).sqrt();
                // [-1, 1] -> [0, 255]
                rot[0] = (((s.rotation[0]/qlen) + 1.0)*0.5 * 255.0) as u8;
                rot[1] = (((s.rotation[1]/qlen) + 1.0)*0.5 * 255.0) as u8;
                rot[2] = (((s.rotation[2]/qlen) + 1.0)*0.5 * 255.0) as u8;
                rot[3] = (((s.rotation[3]/qlen) + 1.0)*0.5 * 255.0) as u8;
            }
        }
        self.buffer = buffer;
    }


    /// Generates a 2D texture from the splats
    pub fn generate_texture(&mut self) { // TODO: parallelize
        if self.buffer.is_empty() {
            return;
        }
        let f_buffer: &[f32] = transmute_slice::<_, f32>(self.buffer.as_slice());
        let u_buffer: &[u8] = transmute_slice::<_, u8>(self.buffer.as_slice());

        let texwidth = 1024*2 as usize;
        let texheight = ((2*self.splat_count) as f64 / texwidth as f64).ceil() as usize;
        let len_texdata = texwidth*texheight*4 as usize; // 4 components per pixel (RGBA)
        log!("Scene::generate_texture(): texheight={}, len_texdata={}", texheight, len_texdata);
        let mut texdata = vec![0_u32; len_texdata];

        {
            let texdata_f = transmute_slice_mut::<_, f32>(texdata.as_mut_slice());
            for i in 0..self.splat_count {
                // x, y, z components of the i-th splat in f_buffer
                let index_f: usize = 8*i;
                texdata_f[index_f + 0] = f_buffer[index_f + 0];
                texdata_f[index_f + 1] = f_buffer[index_f + 1];
                texdata_f[index_f + 2] = f_buffer[index_f + 2];
            }
        }

        {
            let texdata_c = transmute_slice_mut::<_, u8>(texdata.as_mut_slice());
            for i in 0..self.splat_count {
                // r, g, b, a components of the i-th splat in u_buffer
                let index_c: usize = 4*(8*i + 7);
                let index_u: usize = 32*i + 3*4 + 3*4;
                texdata_c[index_c + 0] = u_buffer[index_u + 0];
                texdata_c[index_c + 1] = u_buffer[index_u + 1];
                texdata_c[index_c + 2] = u_buffer[index_u + 2];
                texdata_c[index_c + 3] = u_buffer[index_u + 3];
            }
        }

        for i in 0..self.splat_count {
            let index_f: usize = 8*i;
            let scale = [
                f_buffer[index_f + 3],
                f_buffer[index_f + 4],
                f_buffer[index_f + 5],
            ];

            let index_u: usize = 32*i + 3*4 + 3*4 + 4;
            let rot = [
                // [0, 255] -> [-1, 1]
                ((u_buffer[index_u + 0] as f32)/255.0)*2.0 - 1.0, // qw
                ((u_buffer[index_u + 1] as f32)/255.0)*2.0 - 1.0, // qx
                ((u_buffer[index_u + 2] as f32)/255.0)*2.0 - 1.0, // qy
                ((u_buffer[index_u + 3] as f32)/255.0)*2.0 - 1.0, // qz
            ];

            let r = Mat3::new( // column-major
                1.0 - 2.0*(rot[2]*rot[2] + rot[3]*rot[3]),
                2.0*(rot[1]*rot[2] + rot[0]*rot[3]),
                2.0*(rot[1]*rot[3] - rot[0]*rot[2]),

                2.0*(rot[1]*rot[2] - rot[0]*rot[3]),
                1.0 - 2.0*(rot[1]*rot[1] + rot[3]*rot[3]),
                2.0*(rot[2]*rot[3] + rot[0]*rot[1]),

                2.0*(rot[1]*rot[3] + rot[0]*rot[2]),
                2.0*(rot[2]*rot[3] - rot[0]*rot[1]),
                1.0 - 2.0*(rot[1]*rot[1] + rot[2]*rot[2]),
            );

            let s = Mat3::new(
                scale[0], 0.0, 0.0,
                0.0, scale[1], 0.0,
                0.0, 0.0, scale[2]
            );

            let m = r*s;
            let m = &[ // column-major: [col][row]
                m[0][0], m[0][1], m[0][2],
                m[1][0], m[1][1], m[1][2],
                m[2][0], m[2][1], m[2][2],
            ];

            // M * M^T = R * S * S^T * R^T
            let sigma = [
                m[0]*m[0] + m[3]*m[3] + m[6]*m[6],
                m[0]*m[1] + m[3]*m[4] + m[6]*m[7],
                m[0]*m[2] + m[3]*m[5] + m[6]*m[8],
                m[1]*m[1] + m[4]*m[4] + m[7]*m[7],
                m[1]*m[2] + m[4]*m[5] + m[7]*m[8],
                m[2]*m[2] + m[5]*m[5] + m[8]*m[8],
            ];

            // JavaScript typically uses the host system's endianness
            // (x86-64 and Apple CPUs are little-endian).
            // WASM's linear memory is always little-endian.
            texdata[index_f + 4] = pack_half_2x16(4.0*sigma[0], 4.0*sigma[1]); // a, b
            texdata[index_f + 5] = pack_half_2x16(4.0*sigma[2], 4.0*sigma[3]); // c, d
            texdata[index_f + 6] = pack_half_2x16(4.0*sigma[4], 4.0*sigma[5]); // e, f
        }

        self.tex_data = texdata;
        self.tex_width = texwidth;
        self.tex_height = texheight;
    }


    /// Sorts the splats based on their depth using 16-bit single-pass counting sort
    pub fn sort(scene: &Arc<Self>, view_proj: &[f32], bus: &mut Bus<Vec<u32>>, n_threads: usize) {
        if scene.buffer.is_empty() {
            return;
        }
        let f_buffer: &[f32] = transmute_slice::<_, f32>(scene.buffer.as_slice());

        {
            let mut mutex = scene.prev_vp.lock().unwrap();
            if (*mutex).is_empty() {
                (*mutex).push(view_proj[2]);
                (*mutex).push(view_proj[6]);
                (*mutex).push(view_proj[10]);
            } else {
                let dot =
                    (*mutex)[0]*view_proj[2] +
                    (*mutex)[1]*view_proj[6] +
                    (*mutex)[2]*view_proj[10];
                if (dot - 1.0).abs() < 0.01 {
                    return;
                }
            }
        }

        // calculates the depth for each splat based on the view projection matrix
        // and updates sizeList with the calculated depths.
        let mut max_depth = i32::MIN;
        let mut min_depth = i32::MAX;
        /*
        let mut size_list = vec![0_i32; scene.splat_count];
        for i in 0..scene.splat_count {
            let index_f = 8*i as usize;
            let depth = (
                (
                    view_proj[2] * f_buffer[index_f + 0] +
                    view_proj[6] * f_buffer[index_f + 1] +
                    view_proj[10] * f_buffer[index_f + 2]
                ) * 4096.0
            ) as i32;
            size_list[i] = depth;
            if depth > max_depth { max_depth = depth; }
            if depth < min_depth { min_depth = depth; }
        }
        */
        let size_list: Vec<i32> = (0..scene.splat_count)
            .map(|i| {
                let index_f = 8*i as usize;
                let depth = (
                    (
                        view_proj[2] * f_buffer[index_f + 0] +
                        view_proj[6] * f_buffer[index_f + 1] +
                        view_proj[10] * f_buffer[index_f + 2]
                    ) * 4096.0
                ) as i32;
                if depth > max_depth { max_depth = depth; }
                if depth < min_depth { min_depth = depth; }
                depth
            })
            .collect();
        let mut size_list = size_list;
        //log!("Scene::sort(): max_depth={:?}, min_depth={:?}", max_depth, min_depth);

        let size16: usize = 256*256; // 65,536
        let depth_inv = (size16 - 1) as f32 / (max_depth - min_depth) as f32;

        let mut counts0 = vec![0_u32; size16];
        // count the occurrences of each depth
        for i in 0..scene.splat_count {
            let depth = ((size_list[i] - min_depth) as f32 * depth_inv).floor() as i32;
            let depth = depth.clamp(0, size16 as i32 - 1);
            size_list[i] = depth;
            counts0[depth as usize] += 1;
        }
        let mut starts0 = vec![0_u32; size16];
        // store the cumulative count of elements
        for i in 1..size16 {
            starts0[i] = starts0[i-1] + counts0[i-1];
        }

        let mut depth_index = vec![0_u32; scene.splat_count];
        for i in 0..scene.splat_count {
            let depth = size_list[i] as usize;
            let j = starts0[depth] as usize;
            depth_index[j] = i as u32;
            starts0[depth] += 1;
        }
        depth_index.reverse();// FIXME

        //////////////////////////////////
        // no cloning is happening for the single-consumer case
        let _ = bus.try_broadcast(depth_index);
        //////////////////////////////////

        {
            let mut mutex = scene.prev_vp.lock().unwrap();
            (*mutex)[0] = view_proj[2];
            (*mutex)[1] = view_proj[6];
            (*mutex)[2] = view_proj[10];
        }
    }


    /// Sorts the splats based on their depth using 16-bit single-pass counting sort
    pub fn sort2(scene: &Self, view_proj: &[f32], bus: &mut Bus<Vec<u32>>, n_threads: usize) {
        if scene.buffer.is_empty() {
            return;
        }
        let f_buffer: &[f32] = transmute_slice::<_, f32>(scene.buffer.as_slice());

        {
            let mut mutex = scene.prev_vp.lock().unwrap();
            if (*mutex).is_empty() {
                (*mutex).push(view_proj[2]);
                (*mutex).push(view_proj[6]);
                (*mutex).push(view_proj[10]);
            } else {
                let dot =
                    (*mutex)[0]*view_proj[2] +
                    (*mutex)[1]*view_proj[6] +
                    (*mutex)[2]*view_proj[10];
                if (dot - 1.0).abs() < 0.01 {
                    return;
                }
            }
        }

        // calculates the depth for each splat based on the view projection matrix
        // and updates sizeList with the calculated depths.
        let mut max_depth = i32::MIN;
        let mut min_depth = i32::MAX;
        /*
        let mut size_list = vec![0_i32; scene.splat_count];
        for i in 0..scene.splat_count {
            let index_f = 8*i as usize;
            let depth = (
                (
                    view_proj[2] * f_buffer[index_f + 0] +
                    view_proj[6] * f_buffer[index_f + 1] +
                    view_proj[10] * f_buffer[index_f + 2]
                ) * 4096.0
            ) as i32;
            size_list[i] = depth;
            if depth > max_depth { max_depth = depth; }
            if depth < min_depth { min_depth = depth; }
        }
        */
        let size_list: Vec<i32> = (0..scene.splat_count)
            .map(|i| {
                let index_f = 8*i as usize;
                let depth = (
                    (
                        view_proj[2] * f_buffer[index_f + 0] +
                        view_proj[6] * f_buffer[index_f + 1] +
                        view_proj[10] * f_buffer[index_f + 2]
                    ) * 4096.0
                ) as i32;
                if depth > max_depth { max_depth = depth; }
                if depth < min_depth { min_depth = depth; }
                depth
            })
            .collect();
        let mut size_list = size_list;
        //log!("Scene::sort(): max_depth={:?}, min_depth={:?}", max_depth, min_depth);

        let size16: usize = 256*256; // 65,536
        let depth_inv = (size16 - 1) as f32 / (max_depth - min_depth) as f32;

        let mut counts0 = vec![0_u32; size16];
        // count the occurrences of each depth
        for i in 0..scene.splat_count {
            let depth = ((size_list[i] - min_depth) as f32 * depth_inv).floor() as i32;
            let depth = depth.clamp(0, size16 as i32 - 1);
            size_list[i] = depth;
            counts0[depth as usize] += 1;
        }
        let mut starts0 = vec![0_u32; size16];
        // store the cumulative count of elements
        for i in 1..size16 {
            starts0[i] = starts0[i-1] + counts0[i-1];
        }

        let mut depth_index = vec![0_u32; scene.splat_count];
        for i in 0..scene.splat_count {
            let depth = size_list[i] as usize;
            let j = starts0[depth] as usize;
            depth_index[j] = i as u32;
            starts0[depth] += 1;
        }
        depth_index.reverse();// FIXME

        //////////////////////////////////
        // no cloning is happening for the single-consumer case
        let _ = bus.try_broadcast(depth_index);
        //////////////////////////////////

        {
            let mut mutex = scene.prev_vp.lock().unwrap();
            (*mutex)[0] = view_proj[2];
            (*mutex)[1] = view_proj[6];
            (*mutex)[2] = view_proj[10];
        }
    }

}


/// Loads a .ply or .splat file and returns a [Scene]
pub async fn load_scene() -> Scene {
    /*
    A WebAssembly page has a constant size of 65,536 bytes (or 64KB).
    Therefore, the maximum range that a WASM module can address,
    as WASM currently only allows 32-bit addressing, is 2^16 * 64KB = 4GB.
    */
    let mut scene = Scene::new();

    let file = rfd::AsyncFileDialog::new()
        .add_filter("3DGS model", &["ply", "splat", "spz"])
        .pick_file().await;
    if let Some(f) = file.as_ref() {
        if f.file_name().contains(".ply") {
            let mut file_header_size = 0_u16;
            let mut splat_count = 0_usize;
            let mut cursor = Cursor::new(Vec::<u8>::new());
            let bytes = f.read().await;
            match Scene::parse_file_header(bytes) {
                Ok((fhs, sc, c)) => {
                    file_header_size = fhs;
                    splat_count = sc;
                    cursor = c;
                },
                Err(e) => {
                    log!("load_scene(): ERROR: {}", e);
                    unreachable!();
                },
            }
            scene.splat_count = splat_count;
            scene.load(&mut cursor, file_header_size);

        } else if f.file_name().contains(".splat") {
            scene.buffer = f.read().await;
            scene.splat_count = scene.buffer.len() / 32; // 32bytes per splat

        } else if f.file_name().contains(".spz") {
            let mut spz = Spz::new();
            spz.init();

            let buffer = f.read().await;
            let serialized_splats = load_spz(&mut spz, buffer).await;

            scene.splat_count = serialized_splats.len();
            scene.load_no_normal(serialized_splats);

        } else {
            unreachable!();
        }
    }

    scene.generate_texture();

    log!("load_scene(): scene.splat_count={}", scene.splat_count);

    scene
}


use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use js_sys::{Uint8Array, Object, Boolean};

#[allow(unused_imports)]
use web_sys::{Headers, Request, RequestInit, RequestMode, RequestCredentials, Response, ReadableStream, ReadableStreamDefaultReader};


/// Streams a .splat file via HTTP and returns a [Scene] (blocking, only works in main thread)
pub async fn stream_splat(url: &str) -> Result<Scene, JsValue> {
    let mut scene = Scene::new();

    let mut opts = RequestInit::new();
    opts.method("GET");
    opts.mode(RequestMode::Cors); // cross-origin
    opts.credentials(RequestCredentials::Omit);

    let request = Request::new_with_str_and_init(url, &opts)?;
    let window = web_sys::window().unwrap();

    let res = JsFuture::from(window.fetch_with_request(&request)).await?; // JavaScript Promise execution
    let res: Response = res.dyn_into().unwrap();

    let status = res.status();
    if status != 200 {
        let err = format!("load_splat(): ERROR: HTTP status={}", status);
        log!("{}", err.as_str());
        return Err(JsValue::from_str(err.as_str()));
    }

    let cl = res.headers().get("content-length")?;
    let cl: Result<usize, _> = cl.unwrap().parse();
    let byte_len = cl.unwrap();
    let splat_count = byte_len / 32;
    scene.splat_count = splat_count;
    scene.buffer.resize(byte_len, 0_u8);
    log!("stream_splat(): byte_len={}", byte_len);
    log!("stream_splat(): splat_count={}", splat_count);

    /*
    let array_buffer = JsFuture::from(res.array_buffer()?).await?; // download byte array
    let uint8_array = Uint8Array::new(&array_buffer);
    uint8_array.copy_to(&mut scene.buffer);
    */

    let reader = res.body().unwrap().get_reader();
    let reader: ReadableStreamDefaultReader = reader.dyn_into().unwrap();

    let start = get_time_milliseconds();
    let mut bytes_read: usize = 0;
    loop {
        let result = JsFuture::from(reader.read()).await?;
        let result: Object = result.dyn_into().unwrap();

        let done = js_sys::Reflect::get(&result, &JsValue::from_str("done")).unwrap();
        let done: Boolean = done.dyn_into().unwrap();
        if done.value_of() {
            break;
        }

        let value = js_sys::Reflect::get(&result, &JsValue::from_str("value")).unwrap();
        let value: Uint8Array = value.dyn_into().unwrap();
        let chunk = value.to_vec();

        if bytes_read + chunk.len() <= byte_len {
            scene.buffer[bytes_read..bytes_read+chunk.len()].copy_from_slice(chunk.as_slice());
        } else {
            unreachable!();
        }

        bytes_read += chunk.len();

        //let pct = 100.0*(bytes_read as f64)/(byte_len as f64);
        //log!("stream_splat(): pct={:.2}%", pct);
    }
    let elapsed = 0.001*(get_time_milliseconds() - start);
    log!("stream_splat(): bytes_read={}, byte_len={}, elapsed={:.2}s", bytes_read, byte_len, elapsed);

    scene.generate_texture();

    Ok(scene)
}


use std::{rc::Rc, cell::RefCell};
use web_sys::{Worker, MessageEvent};
use js_sys::Number;


/// Streams a .splat file via HTTP in Worker (non-blocking)
/// Sends downloaded bytes to the main thread via a [Bus]
pub fn stream_splat_in_worker(
    bus_buffer: Rc<RefCell<Bus<Vec::<u8>>>>,
    bus_progress: Rc<RefCell<Bus<f64>>>,
    url: String
) -> Worker {
    let worker_handle = Worker::new("/downloader.js").unwrap();

    let callback_handle = onmessage(bus_buffer, bus_progress);
    worker_handle.set_onmessage(Some(callback_handle.as_ref().unchecked_ref()));

    let url_param = JsValue::from_str(url.as_str());
    worker_handle.post_message(&url_param)
        .expect("stream_splat_in_worker(): ERROR: Failed to post message to worker.");

    callback_handle.forget(); // avoid being dropped prematurely

    worker_handle
}


fn onmessage(
    bus_buffer: Rc<RefCell<Bus<Vec::<u8>>>>,
    bus_progress: Rc<RefCell<Bus<f64>>>
) -> Closure<dyn FnMut(MessageEvent) + 'static> {
    let callback = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data(); // JsValue
        let data: Object = data.dyn_into().unwrap();

        // bytes downloaded
        let bytes = js_sys::Reflect::get(&data, &JsValue::from_str("bytes")).unwrap();
        let bytes: Number = bytes.dyn_into().unwrap();
        let bytes = bytes.value_of() as usize;

        // buffer (fixed length)
        let buffer = js_sys::Reflect::get(&data, &JsValue::from_str("buffer")).unwrap();
        let buffer: Uint8Array = buffer.dyn_into().unwrap();
        let buffer: Vec::<u8> = buffer.to_vec();

        let pct = (bytes as f64)/(buffer.len() as f64);
        //////////////////////////////////
        // non-blocking (i.e., no atomic.wait)
        let mut bus_progress = bus_progress.as_ref().borrow_mut();
        let _ = bus_progress.try_broadcast(pct);
        //////////////////////////////////

        if bytes == buffer.len() {
            log!("onmessage(): splat download complete");
            //////////////////////////////////
            // non-blocking (i.e., no atomic.wait)
            let mut bus_buffer = bus_buffer.as_ref().borrow_mut();
            let _ = bus_buffer.try_broadcast(buffer);
            //////////////////////////////////
        }
    }) as Box<dyn FnMut(_)>);

    callback
}


/*
fn onmessage2(
    bus_buffer: Rc<RefCell<Bus<Vec::<u8>>>>,
    bus_progress: Rc<RefCell<Bus<f64>>>
) -> Closure<dyn FnMut(MessageEvent) + 'static> {
    let callback = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data(); // JsValue
        let data: Object = data.dyn_into().unwrap();

        // content length
        let cl = js_sys::Reflect::get(&data, &JsValue::from_str("conlen")).unwrap();
        let cl: Number = cl.dyn_into().unwrap();
        let cl = cl.value_of() as usize;

        // bytes downloaded
        let bytes = js_sys::Reflect::get(&data, &JsValue::from_str("bytes")).unwrap();
        let bytes: Number = bytes.dyn_into().unwrap();
        let bytes = bytes.value_of() as usize;

        // chunk
        let chunk = js_sys::Reflect::get(&data, &JsValue::from_str("chunk")).unwrap();
        let chunk: Uint8Array = chunk.dyn_into().unwrap();
        let chunk: Vec::<u8> = chunk.to_vec();
        //////////////////////////////////
        // non-blocking (i.e., no atomic.wait)
        let mut bus_buffer = bus_buffer.as_ref().borrow_mut();
        let _ = bus_buffer.try_broadcast(chunk);
        //////////////////////////////////

        let pct = (bytes as f64)/(cl as f64);
        //////////////////////////////////
        // non-blocking (i.e., no atomic.wait)
        let mut bus_progress = bus_progress.as_ref().borrow_mut();
        let _ = bus_progress.try_broadcast(pct);
        //////////////////////////////////

        if bytes == cl {
            log!("onmessage(): splat download complete");

            for _ in 0..10 {
                //////////////////////////////////
                // non-blocking (i.e., no atomic.wait)
                let _ = bus_progress.try_broadcast(1.0);
                //////////////////////////////////
            }
        }
    }) as Box<dyn FnMut(_)>);

    callback
}
*/
