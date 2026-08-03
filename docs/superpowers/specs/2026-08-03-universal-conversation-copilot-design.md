# Universal Conversation Copilot Design

**Date:** 2026-08-03

**Status:** Approved design, ready for implementation planning

**Working product name:** CallerInterview

**Initial release focus:** Interview Mode

## 1. Purpose

CallerInterview will be a universal, real-time conversation copilot for:

- all job seekers, across technical and nontechnical interviews;
- sales professionals during discovery, demos, negotiation, and follow-up;
- meeting participants who need context, notes, decisions, and action tracking;
- presenters who need pacing, question support, and delivery guidance.

The product will combine preparation, live assistance, and post-session improvement in one application. It must match the useful capabilities offered by current interview copilots such as ParakeetAI while providing stronger personalization, broader professional modes, better multilingual behavior, more transparent grounding, and a calmer, more accessible user experience.

Interview Mode is the first/default mode and the first customer-ready release. Sales, Meeting, and Presentation modes reuse the same platform and add mode-specific workflows rather than becoming separate products.

## 2. Product Principles

1. **Useful in the moment.** The first answer scaffold appears quickly enough to help during a live conversation.
2. **Personalized, not generic.** Suggestions use the user's profile, documents, examples, vocabulary, and session context.
3. **Grounded and honest.** The product distinguishes sourced facts, user-authored facts, AI suggestions, and low-confidence inferences.
4. **Calm under pressure.** The live view keeps stable dimensions, avoids layout jumps, and prioritizes the next useful action.
5. **Universal by design.** Modes, languages, accessibility, and professional backgrounds are first-class system concepts.
6. **Private by default.** Raw audio is transient unless the user explicitly elects to retain it. Local-first storage remains usable without cloud sync.
7. **Resilient.** Provider, network, audio, and application failures degrade visibly and recover without losing the session.
8. **Ethically bounded.** The product supports disclosed professional assistance, preparation, accessibility, and note-taking. It does not provide proctoring evasion, process hiding, or false claims that it is undetectable.

## 3. Scope

### 3.1 In Scope

- Windows and macOS desktop applications.
- A web workspace for preparation, review, account, team, and billing workflows.
- Interview, Sales, Meeting, and Presentation mode packs.
- Streaming transcription from microphone and system audio.
- Live question and intent detection.
- Fast answer scaffolds with expandable detailed guidance.
- Resume, job description, story vault, document, website, and session-context retrieval.
- Behavioral, technical, coding, system-design, case, phone, panel, and one-way interview support.
- Multilingual transcription, assistance, translation, and review.
- Notes, bookmarks, session timeline, summaries, coaching, and exports.
- Local-first storage with optional encrypted sync.
- Multiple AI and speech providers through a provider-independent routing layer.
- Free, Pro, Unlimited fair-use, Teams, and bring-your-own-key plans.

### 3.2 Later Extensions

- Mobile companion applications.
- VS Code and Cursor extensions.
- Organization analytics and policy administration.
- Additional mode packs built on the same mode contract.

### 3.3 Explicit Exclusions

- Proctoring bypass or evasion.
- Hiding processes from operating-system or examination controls.
- Automated impersonation or fabricated credentials and experience.
- Silent recording where consent is legally or organizationally required.
- Claims that the application is universally undetectable.

## 4. Market Reference and Parity Target

The following is a product-research snapshot, not a promise that competitor capabilities or pricing remain unchanged.

| Product | Relevant observed strengths | CallerInterview response |
|---|---|---|
| [ParakeetAI](https://www.parakeet-ai.com/) | Live answers, coding screenshot capture, model choice, documents, meeting detection, notes, multilingual support, desktop/web/mobile | Match core live and preparation capabilities; exceed with grounding, review depth, mode packs, accessibility, and mixed-language support |
| [Final Round AI](https://www.finalroundai.com/) | Full interview lifecycle, mock interviews, performance reporting, broad interview categories | Match interview breadth; use one profile and session model from preparation through review |
| [LockedIn AI](https://www.lockedinai.com/) | Live answers, coding support, mock interviews, competency reports, human-helper option, broad language support | Match core assistance and reporting; provide transparent provider routing and evidence provenance |
| [Interviews Chat](https://www.interviews.chat/pricing) | Multiple answer formats, model comparison, web/desktop/extension surfaces, mock practice, multilingual review | Support format presets and provider choice without making live UX cognitively heavy |
| [Sensei](https://www.senseicopilot.com/) | Personal stories, role context, tone customization, industry knowledge, coding | Treat story vault, terminology, and answer style as reusable structured profile data |
| [Beyz](https://beyz.ai/) | Cheat sheets, context management, phone interviews, coding, question bank | Provide structured briefings, phone workflows, and mode-aware knowledge packs |
| [Cluely](https://cluely.com/pricing) | General meeting assistance, real-time help, notes, searchable past meetings, custom prompts and files | Extend beyond interviews through explicit Sales, Meeting, and Presentation modes |

### 4.1 Interview Release Parity Checklist

The first customer-ready Interview release is not complete until it provides:

- dual-source audio capture and reliable streaming transcription;
- automatic question detection and manual prompt fallback;
- personalized answers from resume, job description, stories, and documents;
- concise, STAR, detailed, technical, coding, and executive answer formats;
- behavioral, technical, coding, system-design, case, phone, panel, and one-way workflows;
- coding screenshot ingestion with OCR and structured reasoning;
- multilingual input and response behavior;
- session notes, pins, transcript, review, coaching, and export;
- model/provider selection with automatic failover;
- stable desktop packaging, crash recovery, and privacy controls.

## 5. Product Structure

### 5.1 Shared Application

The application has five primary areas:

1. **Home:** start, resume, or review work.
2. **Prepare:** build context and rehearse.
3. **Live Copilot:** receive real-time assistance.
4. **Review:** inspect, improve, and export outcomes.
5. **Library:** manage profiles, stories, knowledge, sessions, and reusable assets.

Settings, account, billing, privacy, language, and team controls remain globally accessible but do not compete with primary workflows.

### 5.2 Mode Packs

A mode pack supplies:

- preparation fields and readiness rules;
- intent taxonomy and detection prompts;
- retrieval weighting and terminology;
- answer formats and live actions;
- review sections, coaching rubrics, and exports;
- mode-specific consent and compliance guidance.

All modes use the same audio, transcription, retrieval, generation, session, language, account, billing, and sync infrastructure.

### 5.3 Modes

#### Interview

Default mode. Optimized for job seekers and interview practice. Supports behavioral, technical, coding, system-design, case, phone, panel, and one-way interviews.

#### Sales

Supports discovery, qualification, demos, objections, competitive positioning, negotiation, and follow-up. Preparation includes account, contacts, offering, pricing guardrails, case studies, and qualification framework.

#### Meeting

Supports agenda tracking, context recall, decisions, unresolved questions, action ownership, and follow-up. Preparation includes participants, agenda, prior notes, project documents, and desired decisions.

#### Presentation

Supports rehearsal, pacing, speaker notes, anticipated questions, live Q&A, and post-presentation coaching. Preparation includes deck, audience, objective, timing, and risk areas.

## 6. User Experience

### 6.1 Onboarding

Onboarding is progressive and can be completed later. It asks the user to:

1. select intended uses and default mode;
2. select spoken, response, interface, and review languages;
3. import or create a professional profile;
4. import relevant documents;
5. test microphone and system audio;
6. choose local-only or encrypted-sync behavior;
7. calibrate answer length, tone, structure, and assistance level;
8. review recording and consent defaults.

The user can start a temporary session without completing a full profile.

### 6.2 Home

The first viewport presents four direct actions in this order:

1. Start Interview
2. Start Sales Call
3. Start Meeting
4. Start Presentation

It also shows upcoming sessions, recent sessions, unresolved action items, and a quick-session action. Interview is visually primary but the layout must not imply the other modes are add-ons.

### 6.3 Prepare

Prepare uses the selected mode's fields and a common three-part flow:

- **Context:** participants, organization, objective, role, event, and constraints.
- **Knowledge:** profile, documents, stories, websites, notes, and terminology.
- **Briefing:** likely topics, risks, questions, talking points, and readiness checks.

The preparation view produces an editable session brief. Missing information is shown as optional improvements, not blocking errors unless the session requires a specific resource.

### 6.4 Live Copilot

The live surface is a dense, work-focused tool with stable geometry:

- fixed current question or intent region;
- primary fast scaffold, normally three concise points;
- expandable full response or reasoning;
- mode-specific action bar;
- secondary transcript and timeline;
- always-visible audio, language, connection, and provider health;
- notes and pin controls that do not resize the response region;
- keyboard-first navigation and shortcuts;
- separate controls for listening, suggestion generation, and capture.

New transcript text must not force the current answer off-screen. A new detected question creates a new timeline turn while keeping the previous answer recoverable.

### 6.5 Review

Review includes:

- session timeline synchronized to transcript turns;
- editable summary;
- decisions, action items, questions, objections, and follow-ups as appropriate to the mode;
- coaching with cited moments;
- improved versions of selected answers or statements;
- vocabulary and language feedback where requested;
- searchable notes and pins;
- exports to Markdown, PDF, DOCX, and structured JSON where supported.

Review distinguishes original transcript, user edits, and generated content.

## 7. Interview Mode Requirements

### 7.1 Preparation

Interview preparation supports:

- target role and seniority;
- company, team, interviewer, and interview stage;
- job description parsing;
- one or more resume variants;
- portfolio, GitHub, website, and supporting documents;
- reusable story vault with situation, task, action, result, skills, metrics, and evidence;
- company and role research;
- likely question generation;
- mock interview and targeted rehearsal;
- coding language, technical domain, and framework preferences;
- answer-style presets and custom guidance.

### 7.2 Live Answer Formats

The user can choose or switch among:

- concise bullets;
- natural spoken response;
- STAR response;
- detailed response;
- technical explanation;
- coding plan and solution;
- system-design structure;
- case framework;
- executive summary;
- follow-up questions to ask.

The default format is selected by detected question type and user preference. The UI does not display multiple full competing answers unless the user explicitly requests comparison.

### 7.3 Coding Assistance

Coding support includes:

- screenshot or selected-region capture with explicit user action;
- OCR of prompt, examples, constraints, and starter code;
- clarification questions and assumptions;
- solution approach, complexity, edge cases, and test cases;
- code generation in the selected language;
- iterative updates from interviewer feedback;
- optional IDE handoff in a later release.

Generated code must identify assumptions and cannot claim successful execution unless the code was actually run.

### 7.4 Interview Review

Interview review includes:

- question-by-question performance;
- relevance, structure, specificity, evidence, clarity, and concision;
- technical or role-specific rubric where applicable;
- stronger suggested answers grounded in the user's real profile;
- missing stories or profile evidence to add;
- practice queue derived from weak moments;
- thank-you and follow-up draft generation;
- user-controlled readiness and progress trends.

Scores are coaching signals, not claims about an employer's hiring decision.

## 8. Multilingual and Localization Design

Multilingual support is a platform capability shared by every mode.

### 8.1 Language Settings

Each session stores independent values for:

- interface language;
- expected spoken input language or automatic detection;
- preferred suggestion language;
- source-document language;
- review and export language;
- terminology pack and regional variant.

Users can change input and suggestion languages during a live session without restarting it.

### 8.2 Live Language Behavior

The pipeline must support:

- automatic language detection with a visible confidence state;
- manual language locking when automatic detection is wrong;
- code-switching and mixed-language turns;
- accents and regional vocabulary;
- transcription in the original language;
- optional live translation for the user;
- suggestions in the preferred response language;
- optional side-by-side original and translated text;
- pronunciation-friendly response rendering;
- preserved names, numbers, code, product names, and domain terms.

Translation is not inserted between transcription and question detection when doing so would remove useful nuance. The original-language transcript remains canonical; translated text is a derived view.

### 8.3 Knowledge and Retrieval

Documents can be indexed in their original language. Retrieval supports cross-language queries by using multilingual embeddings or query translation while preserving links to original source passages. Generated claims cite the original source and may display a translated excerpt.

User dictionaries allow names, companies, acronyms, technologies, and industry terms to override speech and translation behavior.

### 8.4 Review and Export

Users can review the original transcript, a translated transcript, or both. Summaries and exports can use a different language from the live session. Coaching distinguishes communication structure from language fluency and does not penalize accent or dialect.

### 8.5 Initial Language Coverage

The architecture is language-independent. The implementation plan will define a verified launch matrix based on provider quality, including transcription, generation, translation, mixed-language behavior, and right-to-left rendering. A language is marked supported only after passing its acceptance suite; provider marketing claims alone are insufficient.

### 8.6 Localization and Accessibility

- UI strings use external locale resources from the first Tauri release.
- Layouts support text expansion without truncating controls.
- Right-to-left layout is designed and tested, not simulated by text alignment alone.
- Dates, times, numbers, currencies, and names use locale-aware formatting.
- Keyboard shortcuts account for non-US layouts.
- Screen readers announce language changes and translated regions.

## 9. Technical Architecture

### 9.1 Target Stack

- **Desktop shell:** Tauri 2.
- **Desktop and web UI:** React with TypeScript.
- **Existing runtime reuse:** current Python audio, transcription, retrieval, and AI capabilities retained initially as a managed sidecar.
- **Local data:** encrypted SQLite-compatible database behind repository interfaces.
- **Cloud services:** authentication, entitlements, encrypted sync, usage metering, provider routing, and team administration.
- **Packaging:** signed Windows and macOS installers with automatic update support.

Tauri is preferred over extending the PySide6 monolith because the product needs a reusable web interface, stronger component tooling, accessible UI primitives, and multiple customer surfaces. The Python engine is retained initially to reduce migration risk in audio and real-time behavior. Electron is not the target because the required desktop shell can be delivered with lower runtime overhead through Tauri.

### 9.2 Process Model

The desktop application consists of:

1. **Tauri host:** application lifecycle, window behavior, permissions, secure storage access, updates, and sidecar supervision.
2. **React client:** onboarding, preparation, live, review, library, settings, and account UI.
3. **Python sidecar:** audio capture, voice activity detection, streaming speech recognition, turn detection, retrieval, generation orchestration, OCR adapters, and session event production.
4. **Local database:** durable profiles, sessions, transcripts, settings, knowledge metadata, and sync queue.
5. **Cloud API:** identity, billing, entitlements, encrypted synchronization, organization features, and optional managed model routing.

The host starts, monitors, and restarts the sidecar. A sidecar restart must restore the active session from the event log without duplicating committed turns.

### 9.3 IPC Contract

Desktop-to-sidecar communication uses a typed, versioned, local-only protocol carried as length-prefixed MessagePack frames over the sidecar's inherited standard input and output. The Tauri host owns the subprocess streams and exposes validated commands and events to React; the web view never opens a sidecar port or talks to the process directly. Python owns real-time audio capture, so IPC carries control messages, transcript events, retrieval results, health, and generated guidance rather than raw audio.

Every message contains:

- protocol version;
- message type;
- unique message ID;
- session ID where applicable;
- monotonic sequence number;
- timestamp;
- typed payload;
- correlation ID for requests and responses.

The protocol supports capability negotiation so a newer UI can safely disable features unavailable in an older sidecar. Unknown optional fields are ignored; incompatible major versions fail with a clear update/recovery screen.

### 9.4 Domain Model

Core entities are:

- **Account:** identity, plan, entitlements, and security settings.
- **Workspace:** personal or organizational data boundary.
- **ProfessionalProfile:** roles, skills, preferences, tone, languages, and linked assets.
- **Story:** structured example with evidence, metrics, tags, and language variants.
- **KnowledgeSource:** document, URL, note, profile field, or imported artifact.
- **ModeDefinition:** preparation schema, intents, actions, rubrics, and exports.
- **SessionBrief:** mode-specific context and readiness state.
- **Session:** lifecycle, mode, participants, languages, providers, privacy settings, and health state.
- **TranscriptTurn:** speaker, original text, language, timestamps, confidence, and optional translations.
- **DetectedIntent:** type, confidence, source turns, and user correction.
- **Suggestion:** format, text, provenance, confidence, provider, and generation state.
- **TimelineEvent:** append-only event for transcript, suggestion, note, pin, action, health, and recovery changes.
- **ReviewArtifact:** summary, coaching item, decision, action, improved answer, or export.
- **UsageEvent:** provider-independent metering record without sensitive content.

Identifiers remain stable across local and cloud stores. Personal and organization workspaces never share content implicitly.

## 10. Real-Time Engine

### 10.1 Data Flow

1. Capture microphone and permitted system audio.
2. Apply local device normalization, echo controls, and voice activity detection.
3. Stream audio to the selected speech provider.
4. Produce partial and final transcript turns with language and confidence.
5. Detect speaker role, question boundaries, intent, and mode-specific signals.
6. Retrieve relevant profile, story, document, and prior-turn context.
7. Generate a fast answer scaffold.
8. Optionally expand to a full response or deeper reasoning.
9. Attach provenance and confidence metadata.
10. Append all committed events to the session timeline.
11. Generate review artifacts after or during the session without blocking live assistance.

### 10.2 Latency Strategy

- Partial transcripts can begin retrieval before a turn is final.
- Question boundary detection commits only when confidence or silence threshold is sufficient.
- The fast scaffold uses a constrained prompt and small context budget.
- Detailed generation runs separately and cannot delay the scaffold.
- Common profile and session context is pre-indexed during preparation.
- UI updates stream into fixed regions and do not cause layout shifts.

Target: the first useful scaffold is visible within one second of a finalized question under normal network and provider conditions. End-to-end telemetry separates audio, transcription, detection, retrieval, model, and rendering latency.

### 10.3 Provider Routing

Provider adapters expose capability metadata for languages, streaming, context size, structured output, latency, cost, and regional availability. Routing considers:

- requested language and mode;
- user plan and BYOK settings;
- task type, including fast scaffold versus deep review;
- health, latency, and rate limits;
- privacy and regional constraints;
- estimated cost.

The user can select a provider where the plan permits, but automatic failover is the default. Failover must not silently change a privacy-relevant processing region or send content to a provider the user disabled.

### 10.4 Grounding and Provenance

Suggestions may reference:

- user-authored profile facts;
- imported document passages;
- structured stories;
- session transcript turns;
- explicitly retrieved web research;
- model-generated general guidance.

The UI presents compact source indicators and exposes details on demand. Unsupported personal claims are excluded or clearly presented as a proposed framing for user confirmation.

## 11. Reliability and Error Handling

### 11.1 Audio and Transcription

- Device loss shows an actionable health state and attempts reconnection.
- Switching devices does not end the session.
- Speech-provider interruption retries with bounded backoff and fails over when allowed.
- Buffered audio is bounded and discarded according to the session retention policy.
- Language detection can be corrected without rewriting the original audio or turn history.

### 11.2 Generation and Retrieval

- Fast scaffolds do not wait for optional slow sources.
- Model failure triggers eligible provider failover.
- Weak retrieval produces a visible low-grounding state instead of invented personalization.
- Detailed-answer failure leaves the fast scaffold usable.
- User edits and notes remain local even when cloud services are unavailable.

### 11.3 Application Recovery

- Timeline events are committed incrementally.
- Sidecar and UI crashes restore the latest durable session state.
- In-progress derived content is marked incomplete and can be regenerated.
- Schema and protocol migrations are versioned, reversible where feasible, and backed up before destructive changes.
- Diagnostic logs exclude secrets, raw audio, complete transcript content, and imported document bodies by default.

## 12. Privacy and Security

### 12.1 Data Defaults

- Raw audio is processed transiently and not retained by default.
- Transcripts, suggestions, and notes are stored locally by default.
- Cloud sync is opt-in for personal workspaces and end-to-end encrypted where product functionality permits.
- Screenshots are user-initiated, visibly acknowledged, and retained only according to session settings.
- Recordings require an explicit per-session choice and consent reminder.

### 12.2 Secrets and Identity

- Provider keys and refresh tokens use operating-system credential storage.
- Application databases do not store plaintext API keys.
- Email, Google, Microsoft, and passkey sign-in are supported by the cloud identity layer.
- Local-only use remains available for eligible BYOK workflows without forcing profile content into cloud storage.

### 12.3 User Control

Users can:

- inspect what is stored locally and in the cloud;
- export their data;
- delete a session, knowledge source, profile, workspace, or account;
- configure retention independently for audio, screenshots, transcript, and review artifacts;
- disable specific providers and processing regions;
- review active devices and revoke sessions.

### 12.4 Teams

Teams can manage shared knowledge, templates, policy, entitlements, roles, and billing. Administrators do not gain access to personal profiles, personal story vaults, or private sessions unless the user explicitly moves content into a shared workspace and the interface explains that boundary.

## 13. Accessibility and Responsive Behavior

- Complete keyboard access for onboarding, preparation, live assistance, and review.
- Screen-reader labels, landmarks, live-region restraint, and meaningful focus order.
- User-controlled text size without viewport-width font scaling.
- High-contrast mode and color-independent health states.
- Reduced-motion support.
- Captions and transcript views available without relying on audio.
- Stable dimensions for live controls, transcript, answer areas, counters, and status indicators.
- Compact, standard, and wide desktop layouts with no content overlap.
- Text wraps or expands containers rather than truncating critical content.
- Touch-safe web controls without converting the live desktop surface into a card-heavy mobile layout.

## 14. Accounts, Billing, and Entitlements

### 14.1 Plans

- **Free:** limited monthly live minutes, preparation, and basic reviews.
- **Pro:** higher live limits, full personalization, advanced reviews, and broader providers.
- **Unlimited:** fair-use live assistance with clearly documented limits.
- **Teams:** shared knowledge, policy, seats, administration, and centralized billing.
- **BYOK:** eligible users supply model or speech keys while product entitlements remain separate.

### 14.2 Metering

The product reports understandable minutes and estimated provider usage rather than exposing raw token accounting as the primary customer measure. Usage records include category, duration or units, provider, and cost estimate but exclude transcript text.

Stripe handles subscriptions and invoices. Entitlements are stored independently from Stripe webhook delivery so temporary billing-provider outages do not immediately interrupt an active paid session.

## 15. Migration from the Current Prototype

The current PySide6 application has working value in dual-source audio, Deepgram transcription, DeepSeek generation, document retrieval, role mapping, transcript, pins, notes, summary, modes, shortcuts, and screen-capture behavior. The migration must preserve tested runtime behavior while replacing the customer interface.

Migration approach:

1. define the sidecar protocol around existing runtime capabilities;
2. add an adapter that exposes current Python services without rewriting them;
3. build the Tauri/React shell and shared design system;
4. reproduce existing session behavior through typed IPC and contract tests;
5. migrate durable state into the new repository-backed local schema;
6. retire PySide6 screens only after corresponding Tauri workflows pass parity tests;
7. refactor sidecar internals incrementally after the customer path is stable.

There will be one supported customer UI. The old interface is a temporary migration surface, not a parallel product edition.

## 16. Delivery Sequence

### Phase 1: Foundation

- Tauri 2 and React/TypeScript application shell.
- Shared tokens, components, navigation, responsive behavior, and accessibility baseline.
- Typed/versioned Python sidecar protocol and supervision.
- Encrypted local database, repositories, and append-only session event model.
- Existing-runtime adapter and contract tests.
- Secure credential storage.
- Packaging, updates, diagnostics, and crash recovery.
- Localization infrastructure, locale resources, and bidirectional layout primitives.

### Phase 2: Customer-Ready Interview Mode

- Interview onboarding and profile setup.
- Resume, job description, company, story vault, document, and terminology workflows.
- Audio calibration, dual-source capture, device switching, meeting detection, and visible health.
- Spoken/input, response, interface, and review language controls.
- Redesigned live question, scaffold, answer, transcript, note, and timeline experience.
- Behavioral, technical, coding, system-design, case, phone, panel, and one-way support.
- Coding screenshot and OCR flow.
- Persistent sessions, review, coaching, improved answers, practice queue, and exports.
- Provider choice, failover, BYOK, and usage display.
- Verified launch-language matrix and mixed-language tests.

### Phase 3: Cloud Product

- Account and passkey-capable authentication.
- Opt-in encrypted synchronization.
- Subscription, billing, entitlements, and metering.
- Web preparation, library, review, account, and billing surfaces.
- Device handoff and sync conflict handling.

### Phase 4: Professional Modes

- Sales preparation, live assistance, qualification, objections, and follow-up.
- Meeting agendas, decisions, actions, unresolved questions, and summaries.
- Presentation rehearsal, pacing, speaker guidance, Q&A, and coaching.
- Cross-mode templates and reusable knowledge.

### Phase 5: Extensions

- VS Code and Cursor integrations.
- Mobile companion experiences.
- Organization administration and policy tooling.
- Additional mode packs through the stable mode contract.

## 17. Testing Strategy and Quality Gates

### 17.1 Automated Tests

- Type, lint, and unit tests for React, Tauri, and Python.
- IPC schema compatibility and capability-negotiation tests.
- Repository and migration tests against encrypted local data.
- Recorded-audio transcription and turn-detection fixtures.
- Retrieval provenance and unsupported-claim tests.
- Provider routing, failover, timeout, and rate-limit tests.
- Crash, sidecar restart, and session restore tests.
- Security tests for secret storage, log redaction, workspace boundaries, and deletion.
- Locale tests for text expansion, formatting, right-to-left behavior, and non-US keyboards.
- Language acceptance suites for transcription, mixed-language turns, retrieval, generation, and translation.

### 17.2 Visual and Interaction QA

- Playwright coverage for onboarding, Prepare, Live, Review, Library, and billing.
- Screenshot review at compact, standard, and wide desktop viewports.
- Text-size, high-contrast, reduced-motion, and right-to-left passes.
- Keyboard-only and screen-reader smoke tests.
- Long transcript, long answer, provider failure, audio loss, and language-switch scenarios.
- No overlapping text or controls and no answer-region layout shift.

### 17.3 Release Gates

- First useful scaffold appears within one second after a finalized question under the normal-network benchmark.
- Audio reconnect and device switching do not lose committed session data.
- A new question never destroys or irretrievably scrolls away the prior answer.
- Personalized factual claims are traceable to profile, story, document, or transcript sources.
- Provider failover and offline behavior are visible and preserve user work.
- Crash restoration recovers the latest durable timeline.
- No secrets, raw audio, complete transcripts, or document bodies appear in default diagnostics.
- Windows and macOS installers pass clean-machine launch, update, and uninstall tests.
- Keyboard, screen-reader, text-size, contrast, reduced-motion, and responsive checks pass.
- Every advertised language passes the launch-language acceptance suite.
- Interview parity checklist is complete before secondary modes are declared customer-ready.

## 18. Design Decisions

1. Build one universal application with mode packs, not separate products.
2. Keep Interview first and make it the first fully customer-ready mode.
3. Use Tauri 2 and React/TypeScript for the new customer interface.
4. Preserve the proven Python runtime initially as a supervised sidecar.
5. Use a typed, versioned IPC boundary before refactoring runtime internals.
6. Make original-language transcripts canonical and translations derived.
7. Store locally by default and make cloud synchronization an explicit choice.
8. Show one fast scaffold first, with optional detail, instead of flooding the live view.
9. Require grounding metadata for personalized suggestions.
10. Reject proctoring evasion and dishonest undetectability positioning.

## 19. Success Measures

Product success will be evaluated through:

- activation: users complete audio calibration and a first prepared or quick session;
- live usefulness: low answer latency, high scaffold use, and low manual recovery;
- personalization: grounded story/document usage and reduced generic suggestions;
- reliability: completed sessions without audio, provider, or crash loss;
- review value: review completion, answer improvement, action export, and practice reuse;
- multilingual quality: correction rate, language-switch success, and acceptance-suite performance;
- accessibility: successful keyboard and assistive-technology task completion;
- retention: repeated use across sessions and, later, across more than one mode;
- trust: clear privacy settings, low deletion friction, and low unsupported-claim rate.
