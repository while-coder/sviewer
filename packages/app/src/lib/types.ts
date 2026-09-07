/**
 * 前后端共享的数据类型（与 Rust 侧 serde 结构一一对应）。
 * 本文件不 import 任何模块，是依赖图的最低层。
 */

/** 一条 EXIF 信息：标签名 + 展示值。 */
export interface ExifEntry {
  tag: string
  value: string
}

/** 图片元信息，由 Rust read_image_info 返回。 */
export interface ImageInfo {
  path: string
  fileName: string
  /** 文件字节数 */
  size: number
  width: number
  height: number
  /** 格式名，如 "JPEG"、"PNG" */
  format: string
  exif: ExifEntry[]
}

/** 图片源：url 交给 <img>；bitmap 交给 <canvas> drawImage。 */
export type ImageSource =
  | { kind: 'url'; src: string }
  | { kind: 'bitmap'; bitmap: ImageBitmap }

/** 保存目标格式。original 为原样复制，其余由 Rust 重编码。 */
export type SaveFormat =
  | 'original'
  | 'jpeg'
  | 'png'
  | 'webp'
  | 'bmp'
  | 'tiff'
  | 'gif'
  | 'ico'
  | 'tga'
  | 'ppm'
  | 'qoi'
  | 'ff'
  | 'avif'

/** 裁剪矩形：显示空间（EXIF 归一化 + 旋转/镜像之后）的像素坐标，左上原点。 */
export interface CropRect {
  x: number
  y: number
  w: number
  h: number
}

/** 一条标记笔画：显示空间（EXIF 归一化 + 旋转/镜像后，裁剪前）的像素坐标，保存时由 Rust 烘焙进图片。 */
export interface MarkShape {
  kind: 'rect' | 'ellipse' | 'arrow' | 'pen'
  /** "#rrggbb" */
  color: string
  /** 线宽（显示空间像素） */
  width: number
  /** rect/ellipse/arrow 为对角两点 [起点, 终点]，pen 为折线点集 */
  pts: [number, number][]
}

/** 一次编辑会话参数（对应 Rust ImageEdits；null 字段 = 不做该步）。 */
export interface ImageEdits {
  rotation: number
  flip: boolean
  crop: CropRect | null
  /** 裁剪后的目标宽高 */
  resize: [number, number] | null
  /** 编码质量 1~100，仅 JPEG 消费 */
  quality: number | null
  /** 标记笔画（编辑窗口绘制） */
  marks?: MarkShape[] | null
}

/** 保存结果：实际落盘路径 + 文件字节数。 */
export interface SaveOutcome {
  dest: string
  size: number
}

/** 一个地图打开入口：名称 + URL。 */
export interface MapLink {
  name: string
  url: string
}

/** 一条常用信息（标签 + 中文说明 + 值）。 */
export interface CommonEntry {
  label: string
  value: string
}
