import { Mic2, Phone, Presentation, Users } from "lucide-react";
import type { LucideIcon } from "lucide-react";

export type ModeDefinition = {
  id: "interview" | "sales" | "meeting" | "presentation";
  labelKey: string;
  startLabelKey: string;
  icon: LucideIcon;
  prepareRoute: string;
  releaseState: "available" | "planned";
};

export const modeDefinitions: readonly ModeDefinition[] = [
  {
    id: "interview",
    labelKey: "modes.interview",
    startLabelKey: "home.actions.startInterview",
    icon: Mic2,
    prepareRoute: "/prepare/interview",
    releaseState: "available",
  },
  {
    id: "sales",
    labelKey: "modes.sales",
    startLabelKey: "home.actions.startSales",
    icon: Phone,
    prepareRoute: "/prepare/sales",
    releaseState: "planned",
  },
  {
    id: "meeting",
    labelKey: "modes.meeting",
    startLabelKey: "home.actions.startMeeting",
    icon: Users,
    prepareRoute: "/prepare/meeting",
    releaseState: "planned",
  },
  {
    id: "presentation",
    labelKey: "modes.presentation",
    startLabelKey: "home.actions.startPresentation",
    icon: Presentation,
    prepareRoute: "/prepare/presentation",
    releaseState: "planned",
  },
];
