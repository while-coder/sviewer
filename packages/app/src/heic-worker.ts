/**
 * HEIC/HEIF 解码与编码 Worker。
 *
 * 四种请求，两套输出：
 * - ImageBitmap（主窗口 canvas 直显，走 transfer 零拷贝，最快）：
 *   'bitmap-rgba' Rust 原生解码好的 RGBA 裸像素 / 'bitmap-wasm' libheif WASM 解码整份文件；
 * - WebP blob（编辑窗口等必须用 <img> 的场景，比位图路径多一次重编码）：
 *   'blob-rgba' / 'blob-wasm'，链路同上。
 *
 * 重活全在子线程：12MP 图像也不会冻结 UI。asset URL 由主线程用 convertFileSrc
 * 转好后传入（Worker 里没有 window，拿不到 __TAURI_INTERNALS__）。
 */

/** 主线程发来的解码/编码请求。 */
type DecodeRequest =
  | { id: number; kind: 'bitmap-rgba' | 'blob-rgba'; width: number; height: number; buf: ArrayBuffer }
  | { id: number; kind: 'bitmap-wasm' | 'blob-wasm'; url: string }

type DecodeResponse =
  | { id: number; ok: true; bitmap?: ImageBitmap; blob?: Blob }
  | { id: number; ok: false; error: string }

/** Worker 全局 scope 的 postMessage 需要带 transfer 列表（ImageBitmap 零拷贝转移）。 */
function post(msg: DecodeResponse, transfer: Transferable[] = []) {
  ;(self as unknown as { postMessage: (m: DecodeResponse, t?: Transferable[]) => void }).postMessage(msg, transfer)
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

/** RGBA8 像素 → ImageBitmap（canvas 可直接 drawImage 的 GPU 友好位图）。 */
function toBitmap(width: number, height: number, pixels: Uint8ClampedArray): Promise<ImageBitmap> {
  return createImageBitmap(new ImageData(pixels, width, height))
}

/** RGBA8 像素 → OffscreenCanvas → WebP blob（编码比 PNG 快一个数量级、体积小 ~10 倍）。 */
async function encodeRgba(width: number, height: number, buf: ArrayBuffer): Promise<Blob> {
  const canvas = new OffscreenCanvas(width, height)
  canvas.getContext('2d')!.putImageData(new ImageData(new Uint8ClampedArray(buf), width, height), 0, 0)
  // 源 HEIC 是有损压缩，q0.95 二次编码视觉无差
  return canvas.convertToBlob({ type: 'image/webp', quality: 0.95 })
}

/** libheif WASM 解码 HEIC/HEIF → RGBA 像素（两种输出共用的前半段）。 */
async function decodeWasmPixels(url: string): Promise<{ data: Uint8ClampedArray; width: number; height: number }> {
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
    return await new Promise<{ data: Uint8ClampedArray; width: number; height: number }>(
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
  } finally {
    image.free()
  }
}

async function handle(req: DecodeRequest) {
  try {
    if (req.kind === 'bitmap-rgba') {
      const bitmap = await toBitmap(req.width, req.height, new Uint8ClampedArray(req.buf))
      post({ id: req.id, ok: true, bitmap }, [bitmap])
    } else if (req.kind === 'blob-rgba') {
      const blob = await encodeRgba(req.width, req.height, req.buf)
      post({ id: req.id, ok: true, blob })
    } else {
      const px = await decodeWasmPixels(req.url)
      if (req.kind === 'bitmap-wasm') {
        const bitmap = await toBitmap(px.width, px.height, px.data)
        post({ id: req.id, ok: true, bitmap }, [bitmap])
      } else {
        const blob = await encodeRgba(px.width, px.height, px.data.buffer as ArrayBuffer)
        post({ id: req.id, ok: true, blob })
      }
    }
  } catch (e) {
    post({ id: req.id, ok: false, error: String(e) })
  }
}

self.onmessage = (e: MessageEvent<DecodeRequest>) => {
  void handle(e.data)
}
