import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import arXB from "./locales/ar-XB.json";
import en from "./locales/en.json";

const localeDirections = {
  en: "ltr",
  "ar-XB": "rtl",
} as const;

type SupportedLocale = keyof typeof localeDirections;

function resolveLocale(locale: string): SupportedLocale {
  return locale in localeDirections ? (locale as SupportedLocale) : "en";
}

function setDocumentLocale(locale: SupportedLocale) {
  document.documentElement.lang = locale;
  document.documentElement.dir = localeDirections[locale];
}

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    "ar-XB": { translation: arXB },
  },
  fallbackLng: "en",
  lng: "en",
  interpolation: {
    escapeValue: false,
  },
});

setDocumentLocale("en");

export async function changeInterfaceLanguage(locale: string): Promise<void> {
  const resolvedLocale = resolveLocale(locale);
  await i18n.changeLanguage(resolvedLocale);
  setDocumentLocale(resolvedLocale);
}

export { i18n };
