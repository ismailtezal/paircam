import { clsx, type ClassValue } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';

import { tokens } from '@/theme/tokens';

// The token-driven theme uses its own names for type sizes and adds a hairline border width.
// Without telling tailwind-merge, it would mistake them for colours and drop real colour classes.
const merge = extendTailwindMerge({
  extend: {
    classGroups: {
      'font-size': [{ text: Object.keys(tokens.font.size) }],
      'border-w': [{ border: ['hairline'] }],
      'border-w-b': [{ 'border-b': ['hairline'] }],
    },
  },
});

/** Joins class names and resolves conflicts so later classes win. */
export function cn(...inputs: ClassValue[]) {
  return merge(clsx(inputs));
}
