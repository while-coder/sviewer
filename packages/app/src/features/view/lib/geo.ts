/**
 * 逆地理编码：GPS 坐标 → 一行简略地名（详情抽屉「位置」区块懒加载），
 * 以及各地图 App 的打开链接生成。
 */
import { invoke } from '@tauri-apps/api/core'
import { settings } from '../../../lib/settings'
import type { MapLink } from '../../../lib/types'

export type { MapLink }

/** 由十进制经纬度生成各地图的打开链接。EXIF 坐标为 WGS-84：
 *  高德（默认 GCJ-02）用 coordinate=wgs84、百度（默认 BD-09）用 coord_type=wgs84
 *  声明坐标系，由地图侧转换；Google / Apple 直接用 WGS-84。 */
export function mapLinks(lat: number, lng: number): MapLink[] {
  const pos = `${lng},${lat}`
  return [
    { name: '高德地图', url: `https://uri.amap.com/marker?position=${pos}&coordinate=wgs84&name=图片位置` },
    { name: '百度地图', url: `https://api.map.baidu.com/marker?location=${lat},${lng}&coord_type=wgs84&content=图片位置&output=html&src=sviewer` },
    { name: 'Google 地图', url: `https://www.google.com/maps/search/?api=1&query=${lat},${lng}` },
    { name: 'Apple 地图', url: `https://maps.apple.com/?ll=${lat},${lng}&q=%E5%9B%BE%E7%89%87%E4%BD%8D%E7%BD%AE` },
  ]
}

// ── 地名解析：缓存 / 去重 / 全局限速 ─────────────────────

/** 地名缓存：键含 provider + Key + 四位小数坐标（约 11 米格网），同批同地点只查一次。 */
const geoCache = new Map<string, string | null>()
/** 在途请求去重：同键并发触发（快速切图）只发一次网络请求。 */
const geoPending = new Map<string, Promise<string | null>>()
/** 缓存上限：超出后整体清空（条目极小，看图一次会话撑不到几百个不同地点）。 */
const GEO_CACHE_MAX = 500
/** 相邻两次网络请求的最小间隔。Nominatim 使用政策约 1 次/秒；开着抽屉快速翻图时限速排队。 */
const GEO_MIN_INTERVAL = 1100
/** 请求串行链 + 上次请求时刻：实现全局限速（只影响真正发网络的场合，缓存命中不经过这里）。 */
let geoChain: Promise<unknown> = Promise.resolve()
let geoLastAt = 0

/**
 * 把 GPS 坐标解析成简略地名（如「北京市海淀区中关村大街 1 号」）。
 * 按设置里的 geoProvider 走 Rust command；off / 缺 Key / 网络失败时返回 null，
 * 失败不缓存以便下次打开抽屉重试。只应在打开详情抽屉时调用（切图不触发）。
 */
export async function reverseGeocode(lat: number, lng: number): Promise<string | null> {
  const { geoProvider, amapKey, baiduKey } = settings
  const amap = amapKey.trim()
  const baidu = baiduKey.trim()
  if (geoProvider === 'off') return null
  if (geoProvider === 'amap' && !amap) return null
  if (geoProvider === 'baidu' && !baidu) return null
  const key = `${geoProvider}|${amap}|${baidu}|${lat.toFixed(4)},${lng.toFixed(4)}`
  if (geoCache.has(key)) return geoCache.get(key) ?? null
  const inflight = geoPending.get(key)
  if (inflight) return inflight
  // 过限速链：与上一条请求间隔不足 GEO_MIN_INTERVAL 时补足等待再发
  const p = geoChain
    .then(async () => {
      const wait = GEO_MIN_INTERVAL - (Date.now() - geoLastAt)
      if (wait > 0) await new Promise((r) => setTimeout(r, wait))
      geoLastAt = Date.now()
      return invoke<string | null>('reverse_geocode', {
        lat,
        lng,
        provider: geoProvider,
        amapKey: amap || null,
        baiduKey: baidu || null,
      })
    })
    .then((addr) => {
      if (geoCache.size >= GEO_CACHE_MAX) geoCache.clear()
      geoCache.set(key, addr)
      return addr
    })
    .catch((e) => {
      console.warn('逆地理编码失败', e)
      return null
    })
    .finally(() => geoPending.delete(key))
  // 链上挂「无论如何继续」的版本，单次失败不堵住后续排队
  geoChain = p.catch(() => {})
  geoPending.set(key, p)
  return p
}
