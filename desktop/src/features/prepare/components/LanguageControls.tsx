import { useTranslation } from "react-i18next";
import type { InterviewLanguages } from "../briefSchema";

type LanguageOption = {
  value: string;
  labelKey: string;
  fallback: string;
};

const interfaceLanguageOptions: readonly LanguageOption[] = [
  { value: "en", labelKey: "languages.names.en", fallback: "English" },
];

const contentLanguageOptions: readonly LanguageOption[] = [
  { value: "en", labelKey: "languages.names.en", fallback: "English" },
  { value: "ur", labelKey: "languages.names.ur", fallback: "Urdu" },
  { value: "ar", labelKey: "languages.names.ar", fallback: "Arabic" },
  { value: "hi", labelKey: "languages.names.hi", fallback: "Hindi" },
  { value: "es", labelKey: "languages.names.es", fallback: "Spanish" },
  { value: "fr", labelKey: "languages.names.fr", fallback: "French" },
  { value: "de", labelKey: "languages.names.de", fallback: "German" },
  { value: "pt", labelKey: "languages.names.pt", fallback: "Portuguese" },
];

export type LanguageControlsProps = {
  value: InterviewLanguages;
  onChange(value: InterviewLanguages): void;
  disabled?: boolean;
};

export function LanguageControls({ value, onChange, disabled = false }: LanguageControlsProps) {
  const { t } = useTranslation();

  const update = (field: keyof InterviewLanguages, nextValue: string) => {
    onChange({ ...value, [field]: nextValue });
  };

  return (
    <fieldset className="language-controls" disabled={disabled}>
      <legend>{t("prepare.languages.title", { defaultValue: "Languages" })}</legend>
      <p>{t("prepare.languages.description", {
        defaultValue: "Choose what you hear separately from the language used for suggestions and review.",
      })}</p>

      <div className="language-controls__field">
        <label htmlFor="prepare-ui-language">
          {t("prepare.languages.interface", { defaultValue: "Interface language" })}
        </label>
        <select
          id="prepare-ui-language"
          onChange={(event) => update("ui", event.currentTarget.value)}
          value={value.ui}
        >
          {interfaceLanguageOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.labelKey, { defaultValue: option.fallback })}
            </option>
          ))}
        </select>
      </div>

      <div className="language-controls__field">
        <label htmlFor="prepare-input-language">
          {t("prepare.languages.spoken", { defaultValue: "Spoken language" })}
        </label>
        <select
          id="prepare-input-language"
          onChange={(event) => update("input", event.currentTarget.value)}
          value={value.input}
        >
          <option value="auto">{t("prepare.languages.auto", { defaultValue: "Detect automatically" })}</option>
          {contentLanguageOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.labelKey, { defaultValue: option.fallback })}
            </option>
          ))}
        </select>
      </div>

      <div className="language-controls__field">
        <label htmlFor="prepare-response-language">
          {t("prepare.languages.suggestion", { defaultValue: "Suggestion language" })}
        </label>
        <select
          id="prepare-response-language"
          onChange={(event) => update("response", event.currentTarget.value)}
          value={value.response}
        >
          {contentLanguageOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.labelKey, { defaultValue: option.fallback })}
            </option>
          ))}
        </select>
      </div>

      <div className="language-controls__field">
        <label htmlFor="prepare-review-language">
          {t("prepare.languages.review", { defaultValue: "Review language" })}
        </label>
        <select
          id="prepare-review-language"
          onChange={(event) => update("review", event.currentTarget.value)}
          value={value.review}
        >
          {contentLanguageOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.labelKey, { defaultValue: option.fallback })}
            </option>
          ))}
        </select>
      </div>
    </fieldset>
  );
}
