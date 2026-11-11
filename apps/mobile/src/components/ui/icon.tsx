import { SymbolView, type SymbolViewProps } from 'expo-symbols';

import { tokens, useTheme, type ColorName } from '@/theme';

/** An SF Symbol name. The platform's own icon set keeps the weight and metrics native. */
export type IconName = Extract<SymbolViewProps['name'], string>;

const SIZES = {
  /** Inline with small text. */
  sm: tokens.size.icon,
  /** Inside a control. */
  md: tokens.font.size.lg,
  /** A feature illustration. */
  lg: tokens.font.size.xl,
} as const;

type IconProps = {
  name: IconName;
  size?: keyof typeof SIZES;
  /** A colour token. Icons take their colour from the theme, like text. */
  color?: ColorName;
  weight?: SymbolViewProps['weight'];
};

function Icon({ name, size = 'md', color = 'text', weight = 'medium' }: IconProps) {
  const { colors } = useTheme();
  const points = SIZES[size];
  return (
    <SymbolView
      name={name}
      size={points}
      tintColor={colors[color]}
      weight={weight}
      resizeMode="scaleAspectFit"
      style={{ width: points, height: points }}
      // Icons here always sit next to a label or inside a labelled control.
      accessible={false}
    />
  );
}

export { Icon };
export type { IconProps };
