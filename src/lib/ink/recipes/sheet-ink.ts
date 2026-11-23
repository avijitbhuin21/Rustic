import { n } from '@/lib/ink/paint'
import { defineRecipe } from '@/lib/ink/recipes'
import { clamp } from '@/lib/ink/rng'

export type BindingEdge = 'left' | 'right' | 'top' | 'bottom'
export type BindingKind = 'spiral' | 'holes'

export interface SheetBindingParams {
  edge: BindingEdge
  kind: BindingKind
}

const tok = (name: string) => `var(--ll-${name})`

export const sheetBindingRecipe = defineRecipe<SheetBindingParams>({
  id: 'sheet-binding',
  kind: 'binding',
  layers: ['rest'],
  pad: 26,
  draw(ink, { w, h }, { edge, kind }, ctx) {
    const horizontal = edge === 'top' || edge === 'bottom'
    const along = horizontal ? w : h
    const holeAt = kind === 'holes' ? 17 : 13
    const r = ink.next('binding')
    const num = (v: number) => n(v, 10)
    const map = (v: number, u: number): [number, number] => {
      if (edge === 'left') return [v, u]
      if (edge === 'right') return [w - v, u]
      if (edge === 'top') return [u, v]
      return [u, h - v]
    }
    const pt = (v: number, u: number) => map(v, u).map(num).join(' ')
    const ellipse = (v: number, u: number, rv: number, ru: number) => {
      const [cx, cy] = map(v, u)
      const [rx, ry] = horizontal ? [ru, rv] : [rv, ru]
      return `M${num(cx - rx)} ${num(cy)}a${num(rx)} ${num(ry)} 0 1 0 ${num(2 * rx)} 0a${num(rx)} ${num(ry)} 0 1 0 ${num(-2 * rx)} 0z`
    }

    const spots: number[] = []
    if (kind === 'holes') {
      for (const f of [0.16, 0.5, 0.84]) spots.push(clamp(f * along, 26, along - 26))
    } else {
      const pitch = ctx.calm ? 32 : 28
      const count = Math.max(2, Math.floor((along - 20) / pitch))
      const first = (along - (count - 1) * pitch) / 2
      for (let i = 0; i < count; i++) spots.push(first + i * pitch + r.gauss(0, ctx.calm ? 0 : 0.7))
    }
    const jit = (s: number) => r.gauss(0, s)

    if (kind === 'holes') {
      let disc = ''
      let ring = ''
      let hole = ''
      for (const u of spots) {
        disc += ellipse(holeAt, u, 11.2 + jit(0.2), 11 + jit(0.2))
        ring += ellipse(holeAt, u, 11.4 + jit(0.2), 11.1 + jit(0.2))
        hole += ellipse(holeAt, u, 6.6 + jit(0.2), 6.3 + jit(0.2))
      }
      ink.layer('rest', () => {
        ink.raw(`<path d="${disc}" fill="${tok('photo')}" fill-opacity=".95"/>`, 'top')
        ink.raw(`<path d="${ring}" fill="none" stroke="${tok('pencil')}" stroke-width=".8" stroke-opacity=".55"/>`, 'top')
        ink.raw(`<path d="${hole}" fill="${tok('ink')}" fill-opacity=".78" stroke="${tok('ink')}" stroke-width=".7" stroke-opacity=".8"/>`, 'top')
      })
      ink.grow([0, 0, w, h])
      return
    }

    let holes = ''
    let rims = ''
    let wires = ''
    for (const u of spots) {
      const lean = ctx.calm ? 0 : jit(1.2)
      holes += ellipse(holeAt, u, 5.4 + jit(0.3), 4.6 + jit(0.3))
      const a = map(holeAt - 4.6, u + 2.9)
      const b = map(holeAt + 3.8, u + 3.6)
      const c = map(holeAt - 1, u + 4.6)
      rims += `M${num(a[0])} ${num(a[1])}Q${num(c[0])} ${num(c[1])} ${num(b[0])} ${num(b[1])}`
      const P: Array<[number, number]> = [
        [2.6, 2.4 + jit(0.3)],
        [-2.4, -0.2 + jit(0.3) + lean],
        [-8.5 + jit(0.5), -3.6 + jit(0.4) + lean],
        [-14.5 + jit(0.6), -4.2 + jit(0.4) + lean],
        [-20 + jit(0.6), -1.2 + jit(0.5) + lean],
        [-22.4 + jit(0.5), 2.2 + jit(0.5) + lean],
      ]
      const at = (i: number): string => pt(holeAt + P[i][0], u + P[i][1])
      const mid = (i: number): string => pt(holeAt + (P[i][0] + P[i + 1][0]) / 2, u + (P[i][1] + P[i + 1][1]) / 2)
      wires += `M${at(0)}Q${at(1)} ${mid(1)}Q${at(2)} ${mid(2)}Q${at(3)} ${mid(3)}Q${at(4)} ${at(5)}`
    }
    const wire = `ll-wr${ink.seed.toString(36)}`
    const reuse = (stroke: string, sw: number, op: number, tf: string) =>
      `<use href="#${wire}" fill="none" stroke="${stroke}" stroke-width="${sw}" stroke-opacity="${op}" stroke-linecap="round" stroke-linejoin="round"${tf ? ` transform="${tf}"` : ''}/>`
    ink.layer('rest', () => {
      ink.raw(`<path d="${holes}" fill="${tok('ink')}" fill-opacity=".78"/>`, 'top')
      ink.raw(`<path d="${rims}" fill="none" stroke="${tok('photo')}" stroke-width=".7" stroke-opacity=".7" stroke-linecap="round"/>`, 'top')
      ink.raw(`<defs><path id="${wire}" d="${wires}"/></defs>${reuse(tok('ink'), 1.2, 0.36, 'translate(.3 1.1)')}${reuse(tok('pencil'), 1.7, 1, '')}${reuse(tok('photo'), 0.55, 0.85, 'translate(-.1 -.7)')}`, 'top')
    })
    ink.grow([0, 0, w, h])
  },
})
