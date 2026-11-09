import { useTheme, type ThemePref } from "../theme";
import { Icon, type IconName } from "./icons";

const THEMES: { value: ThemePref; icon: IconName; label: string }[] = [
  { value: "auto", icon: "auto", label: "跟随系统" },
  { value: "light", icon: "sun", label: "日间" },
  { value: "dark", icon: "moon", label: "夜间" },
];

/** 三态主题开关：跟随系统 / 日间 / 夜间。 */
export function ThemeSwitch() {
  const [pref, setPref] = useTheme();
  return (
    <div className="themesw" role="radiogroup" aria-label="主题">
      {THEMES.map((t) => (
        <button
          key={t.value}
          role="radio"
          aria-checked={pref === t.value}
          className={`themesw__item ${pref === t.value ? "is-on" : ""}`}
          onClick={() => setPref(t.value)}
          title={t.label}
          aria-label={t.label}
        >
          <Icon name={t.icon} size={14} />
        </button>
      ))}
    </div>
  );
}
