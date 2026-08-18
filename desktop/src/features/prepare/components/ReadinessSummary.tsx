import { CheckCircle2, Lightbulb } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { InterviewBrief } from "../briefSchema";

type ReadinessGap = "role" | "company" | "resume" | "jobDescription" | "story";

const gapFallbacks: Record<ReadinessGap, string> = {
  role: "Add a target role for more relevant suggestions.",
  company: "Add the company for better context.",
  resume: "Attach a resume to ground answers in your experience.",
  jobDescription: "Attach the job description to focus likely questions.",
  story: "Select a story to strengthen evidence-based answers.",
};

export type ReadinessSummaryProps = {
  brief: InterviewBrief;
};

export function ReadinessSummary({ brief }: ReadinessSummaryProps) {
  const { t } = useTranslation();
  const gaps = readinessGaps(brief);

  return (
    <section aria-label={t("prepare.readiness.title", { defaultValue: "Session readiness" })}>
      <header>
        <CheckCircle2 aria-hidden="true" size={20} strokeWidth={1.8} />
        <div>
          <h3>{t("prepare.readiness.title", { defaultValue: "Session readiness" })}</h3>
          <p>{t("prepare.readiness.ready", {
            defaultValue: "You can start now. Optional context makes suggestions more personal.",
          })}</p>
        </div>
      </header>

      {gaps.length > 0 ? (
        <ul>
          {gaps.map((gap) => (
            <li key={gap}>
              <Lightbulb aria-hidden="true" size={16} strokeWidth={1.8} />
              {t(`prepare.readiness.gaps.${gap}`, { defaultValue: gapFallbacks[gap] })}
            </li>
          ))}
        </ul>
      ) : (
        <p>{t("prepare.readiness.complete", { defaultValue: "Your core interview context is ready." })}</p>
      )}
    </section>
  );
}

export function readinessGaps(brief: InterviewBrief): ReadinessGap[] {
  const gaps: ReadinessGap[] = [];
  if (!brief.context.role) gaps.push("role");
  if (!brief.context.company) gaps.push("company");
  if (!brief.documents.resume) gaps.push("resume");
  if (!brief.documents.jobDescription) gaps.push("jobDescription");
  if (brief.storyIds.length === 0) gaps.push("story");
  return gaps;
}
