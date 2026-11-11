import type { Config } from 'tailwindcss';
import plugin from 'tailwindcss/plugin';

import { tokens, type ColorMode, type ColorName } from './src/theme/tokens';

// Everything in this theme is derived from the generated design tokens. The default Tailwind
// palette, spacing scale, radii and type scale are replaced, not extended, so a class that is not
// backed by a token does not exist.

const kebab = (name: string) => name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);

const px = (values: Record<string, number>) =>
  Object.fromEntries(Object.entries(values).map(([name, value]) => [kebab(name), `${value}px`]));

const colorNames = Object.keys(tokens.color.dark) as ColorName[];

const colorVariable = (name: ColorName) => `--color-${kebab(name)}`;

/** `bg-surface`, `text-text-muted`, `border-border` and so on, resolved per colour scheme. */
const colors = Object.fromEntries(
  colorNames.map((name) => [kebab(name), `var(${colorVariable(name)})`]),
);

const colorVariables = (mode: ColorMode) =>
  Object.fromEntries(colorNames.map((name) => [colorVariable(name), tokens.color[mode][name]]));

const config = {
  content: ['./src/**/*.{ts,tsx}'],
  presets: [require('nativewind/preset')],
  darkMode: 'class',
  theme: {
    colors: {
      transparent: 'transparent',
      ...colors,
    },
    spacing: px(tokens.space),
    borderRadius: {
      none: '0px',
      ...px(tokens.radius),
    },
    fontFamily: {
      sans: [tokens.font.sans],
      mono: [tokens.font.mono],
    },
    fontSize: px(tokens.font.size),
    fontWeight: tokens.font.weight,
    extend: {
      // Control heights and touch targets: `h-control-lg`, `min-h-touch-target`, `size-icon`.
      size: px(tokens.size),
      height: px(tokens.size),
      width: px(tokens.size),
      minHeight: px(tokens.size),
      minWidth: px(tokens.size),
    },
  },
  plugins: [
    plugin(({ addBase }) => {
      addBase({
        ':root': colorVariables('light'),
        '.dark:root': colorVariables('dark'),
      });
    }),
  ],
} satisfies Config;

export default config;
