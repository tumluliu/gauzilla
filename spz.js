// wasm generated from https://github.com/drumath2237/spz-loader/tree/main/packages/core/lib/spz-wasm
import spzwasm from "./lib/spz/spz.mjs";


const floatVectorToFloatArray = (wasmModule, vec, enhancementFunc = (n) => n) => {
    const pointer = wasmModule.vf32_ptr(vec);
    const size = vec.size();
    const buffer = new Float32Array(wasmModule.HEAPF32.buffer, pointer, size);
    const copiedBuffer = buffer.map(enhancementFunc);
    return copiedBuffer;
};


const createGaussianCloudFromRaw = (wasmModule, raw) => {
    return {
        numPoints: raw.numPoints,
        shDegree: raw.shDegree,
        antialiased: raw.antialiased,
        positions: floatVectorToFloatArray(wasmModule, raw.positions),
        scales: floatVectorToFloatArray(wasmModule, raw.scales),
        rotations: floatVectorToFloatArray(wasmModule, raw.rotations),
        alphas: floatVectorToFloatArray(wasmModule, raw.alphas),
        colors: floatVectorToFloatArray(wasmModule, raw.colors),
        sh: floatVectorToFloatArray(wasmModule, raw.sh), // FIXME?
    };
};


const disposeRawGSCloud = (wasmModule, raw) => {
    wasmModule._free(wasmModule.vf32_ptr(raw.positions));
    wasmModule._free(wasmModule.vf32_ptr(raw.scales));
    wasmModule._free(wasmModule.vf32_ptr(raw.rotations));
    wasmModule._free(wasmModule.vf32_ptr(raw.alphas));
    wasmModule._free(wasmModule.vf32_ptr(raw.colors));
    wasmModule._free(wasmModule.vf32_ptr(raw.sh));
};


class SpzSingleton {
    static instance = null;
    static loadingPromise = null;

    static getInstance() {
        if (this.instance) {
            return Promise.resolve(this.instance);
        }

        if (!this.loadingPromise) {
            this.loadingPromise = new Promise(async (resolve, reject) => {
                try {
                    this.instance = await spzwasm();
                    console.log("spz.js: Spz::getInstance(): Wasm module loaded successfully");
                    resolve(this.instance);
                } catch (err) {
                    console.error("spz.js: Spz::getInstance(): Error loading wasm module:", err);
                    reject(err);
                }
            });
        }

        return this.loadingPromise;
    }
}


async function load(url) {
    let pointer = null;

    const instance = await SpzSingleton.getInstance();
    console.log('spz.js: load(): Wasm instance loaded:', instance);

    try {
        // download byte array from url created in main thread
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`spz.js: load(): Failed to fetch data: ${response.status}`);
        }
        URL.revokeObjectURL(url);
        const buffer = await response.arrayBuffer();
        const spzData = new Uint8Array(buffer);
        console.log('spz.js: load(): spzData.length=', spzData.length);

        // let wasm allocate memory for spzData
        pointer = instance._malloc(Uint8Array.BYTES_PER_ELEMENT * spzData.length);
        if (pointer === null) {
            throw new Error("spz.js: load(): couldn't allocate memory");
        }
        instance.HEAPU8.set(spzData, pointer / Uint8Array.BYTES_PER_ELEMENT);

        // generate raw gaussian cloud from spzData
        const rawGsCloud = instance.load_spz(pointer, spzData.length);
        console.log('spz.js: load(): rawGsCloud.numPoints=', rawGsCloud.numPoints);
        //console.log('spz.js: load(): rawGsCloud.shDegree=', rawGsCloud.shDegree);
        //console.log('spz.js: load(): rawGsCloud.antialiased=', rawGsCloud.antialiased);

        let options = null;
        const gaussianCloud = createGaussianCloudFromRaw(instance, rawGsCloud, options);
        disposeRawGSCloud(instance, rawGsCloud);
        console.log('spz.js: load(): gaussianCloud.numPoints=', gaussianCloud.numPoints);
        //console.log('spz.js: load(): gaussianCloud.shDegree=', gaussianCloud.shDegree);
        //console.log('spz.js: load(): gaussianCloud.antialiased=', gaussianCloud.antialiased);
        //console.log('spz.js: load(): gaussianCloud.positions.length=', gaussianCloud.positions.length);
        //console.log('spz.js: load(): gaussianCloud.scales.length=', gaussianCloud.scales.length);
        //console.log('spz.js: load(): gaussianCloud.rotations.length=', gaussianCloud.rotations.length);
        //console.log('spz.js: load(): gaussianCloud.alphas.length=', gaussianCloud.alphas.length);
        //console.log('spz.js: load(): gaussianCloud.colors.length=', gaussianCloud.colors.length);
        //console.log('spz.js: load(): gaussianCloud.sh.length=', gaussianCloud.sh.length);

        // Using transferable objects to avoid data copies
        self.postMessage({
            status: 'loaded',
            gaussianCloud: {
                numPoints: gaussianCloud.numPoints,
                shDegree: gaussianCloud.shDegree,
                antialiased: gaussianCloud.antialiased,
                positions: gaussianCloud.positions.buffer,
                scales: gaussianCloud.scales.buffer,
                rotations: gaussianCloud.rotations.buffer,
                alphas: gaussianCloud.alphas.buffer,
                colors: gaussianCloud.colors.buffer,
                sh: gaussianCloud.sh.buffer,
            }
        }, [
            gaussianCloud.positions.buffer,
            gaussianCloud.scales.buffer,
            gaussianCloud.rotations.buffer,
            gaussianCloud.alphas.buffer,
            gaussianCloud.colors.buffer,
            gaussianCloud.sh.buffer
        ]);

    } catch (e) {
        console.error(e);

    } finally {
        if (pointer !== null) {
            instance._free(pointer);
            console.log('spz.js: load(): done');
        }
    }
}


// Listen for messages from the main thread
self.addEventListener('message', async (e) => {
    const { type, url } = e.data;

    switch (type) {
        case 'load':
            await load(url);
            break;
    }
});
