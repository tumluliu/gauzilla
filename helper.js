export function get_canvas_width() {
    var canvas = document.getElementById("render_canvas");
    var rect = canvas.getBoundingClientRect();
    return rect.width;
}


export function get_canvas_height() {
    var canvas = document.getElementById("render_canvas");
    var rect = canvas.getBoundingClientRect();
    return rect.height;
}


export function cpu_cores() {
    return navigator.hardwareConcurrency;
}


export function get_time_milliseconds() {
    return performance.now();
}


export function get_webgl1_version() {
    const gl = document.createElement("canvas").getContext("webgl");
    return gl.getParameter(gl.VERSION);
}


export function get_webgl2_version() {
    const gl = document.createElement("canvas").getContext("webgl2");
    return gl.getParameter(gl.VERSION);
}


export function get_url_param() {
    const params = new URLSearchParams(location.search);
    if (params.has("url")) {
        let url = params.get("url");
        if (!url.toLowerCase().includes("http")) {
            url = "https://huggingface.co/datasets/satyoshi/gauzilla-data/resolve/main/" + url;
        }
        return url;
    } else {
        return "";
    }
}


function getVectorParam(paramName, defaultValue) {
    const params = new URLSearchParams(window.location.search);
    const param = params.get(paramName);
    if (!param) {
        return defaultValue;  // Default value if the parameter is not found
    }
    const numbers = param.split(',').map(Number);
    if (numbers.length !== 3 || numbers.some(isNaN)) {
        return defaultValue;  // Default value if the parameter is malformed
    }
    return numbers;
}


export function get_position_param() {
    return getVectorParam('position', [0.0, 0.0, 5.0]);
}


export function get_target_param() {
    return getVectorParam('target', [0.0, 0.0, 0.0]);
}


export function get_up_param() {
    return getVectorParam('up', [0.0, 1.0, 0.0]);
}


export async function sleep_js(ms) {
    await new Promise(resolve => setTimeout(resolve, ms));
}
