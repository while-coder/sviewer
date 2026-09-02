/// <reference types="vite/client" />

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}

// libheif-js/wasm-bundle 为 CJS 包，无官方类型；这里声明用到的最小 API 面
declare module 'libheif-js/wasm-bundle' {
  interface HeifImage {
    get_width(): number
    get_height(): number
    /** 把解码后的 RGBA 写入 data，回调里取 d.data 使用；用完调 image.free() */
    display(
      data: { data: Uint8ClampedArray; width: number; height: number },
      callback: (d: { data: Uint8ClampedArray; width: number; height: number }) => void,
    ): void
    free(): void
  }
  const libheif: {
    HeifDecoder: new () => { decode(buffer: Uint8Array | ArrayBuffer): HeifImage[] }
  }
  export default libheif
}
