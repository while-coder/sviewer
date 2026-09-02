/**
 * HEIC/HEIF 解码与编码 Worker。
 *
 * 两种请求：
 * - 'wasm'：libheif WASM 在子线程解码整份文件（原生解码不可用时的兜底）；
 * - 'rgba'：Rust 侧（Windows WIC / macOS Image I/O）已解码好的 RGBA 裸像素，
 *   这里只做像素 → WebP blob 的编码。
 *
 * 编码放子线程：12MP 图像也不会冻结 UI。asset URL 由主线程用 convertFileSrc
 * 转好后传入（Worker 里没有 window，拿不到 __TAURI_INTERNALS__）。
 */

/** 主线程发来的解码/编码请求。 */
type DecodeRequest =
  | { id: number; kind: 'wasm'; url: string }
  | { id: number; kind: 'rgba'; width: number; height: number; buf: ArrayBuffer }

type DecodeResponse =
  | { id: number; ok: true; blob: Blob }
  | { id: number; ok: false; error: string }

/** Worker 全局 scope 的 postMessage 在 DOM lib 类型下需要收窄后调用。 */
function post(msg: DecodeResponse) {
  ;(self as unknown as { postMessage: (m: DecodeResponse) => void }).postMessage(msg)
}

// libheif WASM bundle 体积大，仅首次用到（WASM 兜底路径）时才加载，之后常驻复用
let libheifPromise: Promise<{ HeifDecoder: new () => { decode: (b: Uint8Array) => HeifImage[] } }> | null = null
interface HeifImage {
  get_width(): number
  get_height(): number
  display(cfg: { data: Uint8ClampedArray; width: number; height: number }, cb: (d: { data: Uint8ClampedArray; width: number; height: number }) => void): void
  free(): void
}

function loadLibheif() {
  libheifPromise ??= import('libheif-js/wasm-bundle').then((m) => m.default)
  return libheifPromise
}

/** RGBA8 像素 → OffscreenCanvas → WebP blob（编码比 PNG 快一个数量级、体积小 ~10 倍）。 */
async function encodeRgba(width: number, height: number, buf: ArrayBuffer): Promise<Blob> {
  const canvas = new OffscreenCanvas(width, height)
  canvas.getContext('2d')!.putImageData(new ImageData(new Uint8ClampedArray(buf), width, height), 0, 0)
  // 源 HEIC 是有损压缩，q0.95 二次编码视觉无差
  return canvas.convertToBlob({ type: 'image/webp', quality: 0.95 })
}

/** libheif WASM 解码 HEIC/HEIF → RGBA，再走同一编码出口。 */
async function decodeWasm(url: string): Promise<Blob> {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`读取文件失败：${res.status}`)
  const buf = new Uint8Array(await res.arrayBuffer())

  const libheif = await loadLibheif()
  const images = new libheif.HeifDecoder().decode(buf)
  if (images.length === 0) throw new Error('HEIC 中没有可显示的图片')
  const image = images[0]
  const width = image.get_width()
  const height = image.get_height()

  try {
    const data = await new Promise<{ data: Uint8ClampedArray; width: number; height: number }>(
      (resolve, reject) => {
        image.display(
          { data: new Uint8ClampedArray(width * height * 4), width, height },
          (d) => {
            if (d) resolve(d)
            else reject(new Error('libheif 解码失败'))
          },
        )
      },
    )
    return await encodeRgba(data.width, data.height, data.data.buffer as ArrayBuffer)
  } finally {
    image.free()
  }
}

async function handle(req: DecodeRequest) {
  try {
    const blob =
      req.kind === 'rgba'
        ? await encodeRgba(req.width, req.height, req.buf)
        : await decodeWasm(req.url)
    post({ id: req.id, ok: true, blob })
  } catch (e) {
    post({ id: req.id, ok: false, error: String(e) })
  }
}

self.onmessage = (e: MessageEvent<DecodeRequest>) => {
  void handle(e.data)
}
