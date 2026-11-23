
import type { ArtDef } from '@/lib/ink/art/types'
import { nature } from '@/lib/ink/art/doodles/nature'
import { things } from '@/lib/ink/art/doodles/things'
import { marks } from '@/lib/ink/art/doodles/marks'

export const doodleBatches = { nature, things, marks } as const
export const doodles = { ...nature, ...things, ...marks } satisfies Record<string, ArtDef>
export type DoodleName = keyof typeof doodles
