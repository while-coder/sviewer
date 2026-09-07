/**
 * EXIF 展示层：标签中文翻译、GPS 坐标解析、常用信息挑选。
 * 只做纯数据处理，不涉及网络（逆地理编码见 geo.ts）。
 */
import type { CommonEntry, ExifEntry } from '../../../lib/types'

/** EXIF 标签名 → 中文。未命中的保留原名。 */
const EXIF_ZH: Record<string, string> = {
  Make: '制造商',
  Model: '相机型号',
  Software: '软件',
  Orientation: '方向',
  XResolution: 'X 分辨率',
  YResolution: 'Y 分辨率',
  ResolutionUnit: '分辨率单位',
  DateTime: '修改时间',
  DateTimeOriginal: '拍摄时间',
  DateTimeDigitized: '数字化时间',
  OffsetTime: '时区',
  OffsetTimeOriginal: '拍摄时区',
  OffsetTimeDigitized: '数字化时区',
  SubSecTime: '亚秒',
  SubSecTimeOriginal: '拍摄亚秒',
  SubSecTimeDigitized: '数字化亚秒',
  ExposureTime: '曝光时间',
  ShutterSpeedValue: '快门速度',
  FNumber: '光圈值',
  ApertureValue: '光圈',
  MaxApertureValue: '最大光圈',
  BrightnessValue: '亮度',
  ExposureBiasValue: '曝光补偿',
  ExposureProgram: '曝光程序',
  ExposureMode: '曝光模式',
  PhotographicSensitivity: '感光度',
  ISOSpeedRatings: '感光度',
  ISOSpeed: '感光度',
  MeteringMode: '测光模式',
  Flash: '闪光灯',
  FocalLength: '焦距',
  FocalLengthIn35mmFilm: '等效焦距',
  LensMake: '镜头制造商',
  LensModel: '镜头型号',
  LensSpecification: '镜头规格',
  LensSerialNumber: '镜头序列号',
  WhiteBalance: '白平衡',
  ColorSpace: '色彩空间',
  SceneCaptureType: '场景类型',
  SceneType: '场景类型',
  SensingMethod: '感应方式',
  Contrast: '对比度',
  Saturation: '饱和度',
  Sharpness: '锐度',
  SubjectDistance: '对焦距离',
  SubjectArea: '对焦区域',
  ExifVersion: 'EXIF 版本',
  FlashpixVersion: 'FlashPix 版本',
  PixelXDimension: '有效宽度',
  PixelYDimension: '有效高度',
  BodySerialNumber: '机身序列号',
  SerialNumber: '机身序列号',
  Artist: '作者',
  Copyright: '版权',
  ImageDescription: '描述',
  UserComment: '备注',
  GPSLatitudeRef: 'GPS 纬度参考',
  GPSLatitude: 'GPS 纬度',
  GPSLongitudeRef: 'GPS 经度参考',
  GPSLongitude: 'GPS 经度',
  GPSAltitudeRef: 'GPS 高度参考',
  GPSAltitude: 'GPS 高度',
  GPSTimeStamp: 'GPS 时间',
  GPSDateStamp: 'GPS 日期',
  // GPS 扩展字段（kamadak-exif 的 GPS 标签组）
  GPSVersionID: 'GPS 版本',
  GPSMapDatum: 'GPS 地图基准',
  GPSPositioningError: 'GPS 定位误差',
  GPSProcessingMethod: 'GPS 处理方法',
  GPSAreaInformation: 'GPS 区域信息',
  GPSDifferential: 'GPS 差分修正',
  GPSDOP: 'GPS 精度',
  GPSSpeedRef: 'GPS 速度单位',
  GPSSpeed: 'GPS 速度',
  GPSTrackRef: 'GPS 移动方向参考',
  GPSTrack: 'GPS 移动方向',
  GPSImgDirectionRef: 'GPS 朝向参考',
  GPSImgDirection: 'GPS 朝向',
  GPSDestLatitudeRef: 'GPS 目的地纬度参考',
  GPSDestLatitude: 'GPS 目的地纬度',
  GPSDestLongitudeRef: 'GPS 目的地经度参考',
  GPSDestLongitude: 'GPS 目的地经度',
  GPSDestBearingRef: 'GPS 目地方向参考',
  GPSDestBearing: 'GPS 目地方向',
  GPSDestDistanceRef: 'GPS 目的地距离单位',
  GPSDestDistance: 'GPS 目的地距离',
  // iPhone 常见的苹果扩展 / 复合图像标签
  CompositeImage: '复合图像',
  SourceImageNumberOfCompositeImage: '复合图像源数量',
  SourceExposureTimesOfCompositeImage: '复合图像曝光时间',
  CameraOwnerName: '相机所有者',
  ImageUniqueID: '图像唯一 ID',
  OwnerName: '所有者',
}

/** EXIF 标签的中文显示名。 */
export function exifLabel(tag: string): string {
  return EXIF_ZH[tag] ?? tag
}

/** 去掉 libheif/kamadak 展示值两端的引号。 */
function cleanExifValue(value: string): string {
  return value.replace(/^"(.*)"$/s, '$1')
}

/** 解析单个 GPS 坐标值：兼容 kamadak 的有理数展示 "36/1, 6/1, 1230/100"
 *  与度分秒展示 `36° 6' 12.30"` 两种格式，返回十进制度。 */
function parseCoord(value: string): number | null {
  const parts = [...value.matchAll(/(\d+(?:\.\d+)?)(?:\/(\d+(?:\.\d+)?))?/g)]
  if (parts.length === 0) return null
  const nums = parts.slice(0, 3).map((m) => {
    const n = parseFloat(m[1])
    return m[2] ? n / parseFloat(m[2]) : n
  })
  return nums[0] + (nums[1] ?? 0) / 60 + (nums[2] ?? 0) / 3600
}

/** 从 EXIF 里提取 GPS 经纬度（十进制度，WGS-84）；无 GPS 或解析失败返回 null。 */
export function parseGpsCoord(exif: ExifEntry[]): { lat: number; lng: number } | null {
  const find = (tag: string) => {
    const e = exif.find((x) => x.tag === tag)
    return e ? cleanExifValue(e.value) : null
  }
  const latRaw = find('GPSLatitude')
  const lngRaw = find('GPSLongitude')
  if (!latRaw || !lngRaw) return null
  let lat = parseCoord(latRaw)
  let lng = parseCoord(lngRaw)
  if (lat == null || lng == null) return null
  // 南纬/西经为负；参考方向缺失时按北纬东经处理
  if ((find('GPSLatitudeRef') ?? 'N').toUpperCase().startsWith('S')) lat = -lat
  if ((find('GPSLongitudeRef') ?? 'E').toUpperCase().startsWith('W')) lng = -lng
  return { lat, lng }
}

/** 从完整 EXIF 里挑常用拍摄信息（按优先级取第一个命中的 tag）。 */
export function pickCommonInfo(exif: ExifEntry[]): CommonEntry[] {
  const find = (...tags: string[]) => {
    for (const tag of tags) {
      const e = exif.find((x) => x.tag === tag)
      if (e && e.value.trim()) return cleanExifValue(e.value)
    }
    return null
  }
  const out: CommonEntry[] = []
  const push = (label: string, ...tags: string[]) => {
    const v = find(...tags)
    if (v) out.push({ label, value: v })
  }
  push('相机', 'Model')
  push('镜头', 'LensModel')
  push('拍摄时间', 'DateTimeOriginal', 'DateTime')
  push('感光度', 'PhotographicSensitivity', 'ISOSpeedRatings', 'ISOSpeed')
  push('光圈', 'FNumber')
  push('快门', 'ExposureTime')
  push('焦距', 'FocalLength')
  push('等效焦距', 'FocalLengthIn35mmFilm')
  push('曝光补偿', 'ExposureBiasValue')
  push('白平衡', 'WhiteBalance')
  push('闪光灯', 'Flash')
  push('软件', 'Software')
  return out
}
