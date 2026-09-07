/** 「保存到原图」按钮的可用性与提示文案（主窗口与编辑窗口共用）。 */
import { computed, type Ref } from 'vue'
import { extOf, extSupportsEdit } from '../lib/formats'
import type { SaveFormat } from '../lib/types'

export function useSaveability(options: {
  /** 当前图片路径 */
  path: Ref<string | null>
  /** 是否有未保存的编辑 */
  modified: () => boolean
  /** 输出偏好（非 original 时按钮禁用，提示走另存为） */
  outputFormat: () => SaveFormat
  /** 「无编辑」时的提示文案（两窗口编辑项不同） */
  noEditHint: string
}) {
  /** 能否「保存到原图」：有编辑、目标格式=原格式、且该扩展名可编码写回。 */
  const canSaveEdits = computed(() => {
    const p = options.path.value
    if (!p || !options.modified()) return false
    if (options.outputFormat() !== 'original') return false
    return extSupportsEdit(p)
  })
  const saveBtnTitle = computed(() => {
    const p = options.path.value
    if (!p) return '保存'
    if (!options.modified()) return options.noEditHint
    if (options.outputFormat() !== 'original') return '保存（已选目标格式，请用「另存为…」）'
    if (!extSupportsEdit(p))
      return `保存（.${extOf(p)} 不支持直接修改，可用「另存为」转换格式）`
    return '保存（写回原图）'
  })
  return { canSaveEdits, saveBtnTitle }
}
