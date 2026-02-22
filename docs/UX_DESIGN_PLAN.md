# DockerVault – UX/UI Design Plan

> **Erstellt:** 22.02.2026
> **Status:** Analyse & Verbesserungsplan
> **Scope:** Gesamte Anwendung (Frontend + User-Flows)

---

## Inhaltsverzeichnis

1. [Zusammenfassung der aktuellen Probleme](#1-zusammenfassung)
2. [Backup Wizard – Detailanalyse & Verbesserungen](#2-backup-wizard)
3. [Seiten-spezifische UX-Probleme](#3-seiten-spezifisch)
4. [Globale UX-Inkonsistenzen](#4-globale-inkonsistenzen)
5. [Empfohlene User-Flows](#5-user-flows)
6. [Priorisierte Umsetzungs-Roadmap](#6-roadmap)
7. [Design-Tokens & Patterns](#7-design-tokens)

---

## 1. Zusammenfassung

### Bereits umgesetzt (in unstaged Changes)
- ✅ Smart-Defaults für Schedule (1 vorhanden → auto-select)
- ✅ Smart-Defaults für Retention Policy (1 vorhanden → auto-select)
- ✅ Smart-Defaults für Remote Storage (1 vorhanden → auto-select)
- ✅ Suche in Dependencies-Step
- ✅ Container-Gruppierung nach Compose-Project in Dependencies
- ✅ Per-Volume Path Rules (statt nur globale Include/Exclude)
- ✅ Step-Skipping für Volume/Path Targets (Steps 2+3)
- ✅ Keyboard Navigation (Enter/Escape) im Wizard
- ✅ ConfirmDialog statt `window.confirm()`
- ✅ Search & Filter auf Backups-Seite
- ✅ Remote Storage IDs beim Editieren korrekt übernehmen

### Noch offene UX-Probleme

Die Anwendung hat systematische Schwächen in folgenden Bereichen:

| Bereich | Schwere | Beschreibung |
|---------|---------|-------------|
| Wizard Step-Relevanz | Hoch | Steps werden angezeigt die für den Kontext irrelevant sind |
| Informationsdichte | Hoch | User hat keinen Überblick über Mengenangaben (wie viele Container, Volumes etc.) |
| Daten-Vorabfüllung | Hoch | Beim Starten des Wizards von einem bestimmten Item wird nichts vorausgefüllt |
| Navigation-Feedback | Mittel | User weiß nicht wo er im Gesamtprozess steht |
| Leere Zustände | Mittel | Empty States sind inkonsistent und nicht hilfreich |
| Bestätigungen | Niedrig | ConfirmDialog fehlt noch an einigen Stellen (Retention) |
| Responsiveness | Niedrig | Mobile Ansicht nicht optimiert |

---

## 2. Backup Wizard – Detailanalyse & Verbesserungen

### 2.1 Step 1: Target Select

**Aktueller Stand:** Gut – SearchableList mit Suche, Sortierung, Status-Badges.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| T1 | Wenn Wizard von Backups-Seite für ein bestimmtes Item geöffnet wird (z.B. "Set Up Backup" bei einem Container), wird der Typ und das Target nicht vorausgefüllt | `BackupWizard` Props um `preselectedType` und `preselectedTarget` erweitern. Wenn gesetzt: Target-Typ automatisch setzen, Item auswählen, direkt zu Step 2 springen | Hoch |
| T2 | Backup-Name wird generisch generiert (`Backup: container_name`) | Intelligentere Namens-Generierung basierend auf dem Kontext. Stack: `Stack: {name} Daily` Container: `{name} Backup` | Niedrig |
| T3 | Kein visueller Hinweis, ob für dieses Target bereits ein Backup konfiguriert ist | Badge "Already configured" an Items zeigen, die bereits ein Backup-Target haben. Verhindert doppelte Konfiguration | Mittel |
| T4 | Bei Stack-Auswahl sieht man nur Container- und Volume-Anzahl, aber nicht welche Volumes | Expandierbares Detail-Panel: Klick auf Stack zeigt Container-Liste und Volume-Liste | Niedrig |

### 2.2 Step 2: Volume Configuration

**Aktueller Stand:** Gut nach den gestaged Änderungen – Per-Volume Rules, PathEditor-Komponent.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| V1 | Bei Stacks: Die globalen Include/Exclude Rules machen wenig Sinn weil jedes Volume andere Daten hat | UX-Anpassung: Bei Stacks die "Default Path Rules" Sektion visuell weniger prominent machen (collapsed by default) und die Per-Volume Rules als primäre Interaktion hervorheben. Hinweistext: "For stacks, configure rules per volume for best results" | Hoch |
| V2 | "Select All" / "Select None" Buttons könnten verwirren, weil "All" = leeres Array (alle) vs explizit alle auswählen | Klarere Labeling: "Backup all volumes" (Default) vs "Select specific volumes". Radio-Button-artige Auswahl oben, dann darunter die Checkboxen nur wenn "Select specific" gewählt | Mittel |
| V3 | Volume-Namen sind oft kryptisch (z.B. `stackname_db-data`) | Volume-Labels oder zugehörigen Container-Namen als Sublabel anzeigen. Mapping: Welcher Container nutzt welches Volume | Mittel |
| V4 | Kein Hinweis auf die ungefähre Größe der Volumes | Volume-Size (Docker API `docker system df -v`) als Info anzeigen, damit User weiß wie groß das Backup wird | Niedrig |

### 2.3 Step 3: Dependencies

**Aktueller Stand:** Gut nach den gestaged Änderungen – Suche, Gruppierung, All/None Buttons.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| D1 | Bei Stacks: Man sieht alle Container des Systems, aber für einen Stack sind primär die Stack-eigenen Container relevant | Zwei Bereiche: (1) "Stack-Container" – vorausgewählt, oberhalb, mit Info "These containers belong to this stack". (2) "Other containers" – darunter, collapsed, für externe Dependencies | Hoch |
| D2 | Die Stop/Start-Reihenfolge ist aktuell eine flache Liste, aber bei Stacks sollte die Reihenfolge aus `depends_on` abgeleitet werden | Auto-detect Button existiert bereits. Verbessern: Bei Stack automatisch ausführen (nicht nur Button), Ergebnis visuell als Dependency-Graph/Tree anzeigen statt flacher Liste | Mittel |
| D3 | "Select all" wählt ALLE Container aus, auch die die nichts mit dem Backup zu tun haben | Bei Stacks: "Select all" soll nur Stack-Container vorauswählen. Separater "Select all external" Button für andere Container | Mittel |
| D4 | Die Reihenfolge der Dependencies (Stop/Start) kann der User nicht anpassen | Drag & Drop für die Reihenfolge der Dependencies, oder zumindest Up/Down Buttons | Niedrig |

### 2.4 Step 4: Schedule

**Aktueller Stand:** Gut nach den Smart-Default Änderungen.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| S1 | Bei mehreren Schedules: Kein visueller Unterschied welcher "passen" könnte | Schedule-Cards: Zeigen wie viele Targets den Schedule bereits nutzen. Sortierung: Meistgenutzte zuerst. Optional: "Recommended" Badge für den aktivsten Schedule | Mittel |
| S2 | Cron-Expression Vorschau fehlt: User sieht `0 3 * * *` aber versteht nicht was das konkret bedeutet | Human-readable Cron-Vorschau unter der Expression: "Every day at 03:00" (cronstrue-Library oder eigene Übersetzung). Plus: "Next 3 runs" Vorschau mit konkreten Daten/Zeiten | Hoch |
| S3 | "Create new schedule" und "None" sind gleichwertig positioniert, aber "None" ist fast immer die falsche Wahl | "None" kleiner darstellen, ans Ende verschieben. Visuell abgrenzen mit Warnung-Styling. Tooltip: "Only for manual backups" | Niedrig |
| S4 | Presets sind hinter einem Toggle versteckt | Presets direkt sichtbar beim Erstellen eines neuen Schedules (nicht hinter ChevronDown), da sie die häufigste Interaktion sind | Mittel |

### 2.5 Step 5: Remote Storage

**Aktueller Stand:** Gut nach Smart-Default Änderung.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| RS1 | "Configure Storage" Link führt zu `/storage` und verlässt den Wizard – alles geht verloren | Stattdessen: Link öffnet in neuem Tab (`target="_blank"`) ODER ein Inline-Hinweis "Save your current wizard progress first, then configure storage" ODER Storage-Konfiguration inline im Wizard ermöglichen | Hoch |
| RS2 | Wenn kein Storage konfiguriert ist, ist die leere Ansicht sehr prominent (großes Cloud-Icon) obwohl der Step optional ist | Kompakterer Empty-State: Kleiner Hinweis "No remote storage configured. Backups will be stored locally only." mit optional "Set up storage" Link | Niedrig |
| RS3 | Multiselect-Logik (mehrere Storages gleichzeitig) ist nicht offensichtlich | Expliziter Hinweis: "You can select multiple storage locations" mit Checkboxen statt Toggle-Buttons | Niedrig |

### 2.6 Step 6: Retention Policy

**Aktueller Stand:** Gut nach Smart-Default Änderung.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| R1 | Bei "Create new policy": Die numerischen Inputs haben keinen visuellen Indikator was die Zahlen bedeuten | Visuelle Retention-Vorschau: "With this policy, ~X backups will be kept" Berechnung basierend auf dem gewählten Schedule | Mittel |
| R2 | Presets setzen nur die Werte, aber nicht den Namen | Preset-Auswahl sollte auch den Policy-Namen vorausfüllen (z.B. "Keep Last 7" → name = "Keep Last 7") | Niedrig |
| R3 | "None" Warnung sagt nur "all backups will be kept indefinitely" | Konkretere Warnung: "Without a retention policy, disk space usage will grow continuously. We recommend at least a 'Keep Last 7' policy." | Niedrig |

### 2.7 Step 7: Options

**Aktueller Stand:** Funktional, aber wenig besucht.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| O1 | Die "Advanced Options" sind hinter einem Toggle versteckt. Pre/Post-Commands sind Power-User Feature und korrekt versteckt. Compression ist aber ein normales Feature | Compression aus den Advanced Options herausnehmen und als eigene prominente Sektion darstellen (wie aktuell). Pre/Post-Commands hinter "Advanced" lassen. Ggf. den ganzen "Options" Step als optional kennzeichnen | Niedrig |
| O2 | Keine Erklärung was die Compression-Optionen konkret bedeuten für den User | Tooltip oder Sublabel: "Gzip: ~60% smaller, fast" / "Zstd: ~70% smaller, slightly slower" | Niedrig |

### 2.8 Step 8: Summary

**Aktueller Stand:** Gute Übersicht mit Status-Indikatoren.

**Verbesserungen:**

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| SU1 | Kein "Edit" Pro-Section – wenn man was ändern will muss man manuell zurück navigieren | Jede Section bekommt einen kleinen "Edit" (Pencil) Button der direkt zum entsprechenden Step springt | Mittel |
| SU2 | Volume-Konfiguration und Per-Volume-Rules werden nicht in der Summary angezeigt | Summary Section für Volume-Config hinzufügen: "Volumes: 3/5 selected, 2 with custom rules" | Mittel |
| SU3 | Remote Storage IDs werden nicht in die Summary einbezogen | ✅ Ist schon implementiert (selectedStorages wird angezeigt) | - |

### 2.9 Wizard – Übergreifende Verbesserungen

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| W1 | Der Wizard ist ein Modal – bei vielen Steps fühlt sich das einengend an | Optionale Fullscreen-Variante des Wizards, oder zumindest `max-w-5xl` statt `max-w-4xl` für mehr Platz | Niedrig |
| W2 | Kein "Draft/Save Progress" – wenn man den Wizard schließt geht alles verloren | LocalStorage Persistierung des Wizard-State. Beim erneuten Öffnen fragen: "Continue where you left off?" | Mittel |
| W3 | Die Progress-Stepper-Bubbles zeigen nur Nummern, man muss sich erinnern was Step 4 ist | Step-Labels unter den Bubbles anzeigen (abgekürzt: "Target", "Vols", "Deps", "Sched", "Store", "Ret", "Opts", "Done") | Mittel |
| W4 | Skipped Steps (Vol/Deps bei Volume/Path) zeigen "-" in der Bubble – unklar warum | Tooltip auf den skipped Steps: "Not applicable for volume backups" (schon implementiert). Zusätzlich: Die skipped Bubbles visuell kleiner machen oder ganz ausblenden | Niedrig |
| W5 | Beim Editieren eines Targets springt der Wizard nicht zu den geänderten Feldern | Beim Edit-Modus: Alle Steps als "visited" markieren (grüne Checks), User kann direkt zu jedem Step springen | Mittel |

---

## 3. Seiten-spezifische UX-Probleme

### 3.1 Dashboard

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| DA1 | Dashboard zeigt nur rohe Zahlen (Container-Count, Volume-Count) ohne Kontext | Kontext hinzufügen: "5/12 containers backed up", "3 volumes unprotected" – actionable Insights statt nur Zahlen | Hoch |
| DA2 | Keine Quick-Actions: User muss zur Backups-Seite navigieren um ein Backup zu starten | "Quick Backup" Button auf dem Dashboard. Oder: Klick auf einen Recent-Backup-Eintrag → Details/Actions | Mittel |
| DA3 | "Recent Backups" zeigt nur die letzten 5 ohne Filter | "View All" Link der zur Backups-Seite navigiert mit einem Backup-History Filter | Niedrig |
| DA4 | Kein visueller Hinweis wenn es Probleme gibt (failed Backups, disconnected, etc.) | Alert-Banner oben bei Problemen: "⚠ 2 backups failed in the last 24h" mit Link zur Detail-Ansicht | Hoch |
| DA5 | Storage-Platz Info fehlt | Disk-Usage Anzeige: "Backup Storage: 12.5 GB used" mit optional Chart über die Zeit | Mittel |

### 3.2 Backups-Seite (ehem. Targets)

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| B1 | Target-Cards sind alle eingeklappt – man muss jede einzeln aufklappen um den Status zu sehen | Last-Backup-Status (Erfolgreich/Fehlgeschlagen/Datum) direkt auf der eingeklappten Card sichtbar machen | Hoch |
| B2 | Keine Sortierung der Targets | Sortierung nach: Name, Typ, Letztes Backup, Status, Erstellt. Default: Letztes Backup (neuestes zuerst) | Mittel |
| B3 | "Run Now" Button gibt kein visuelles Feedback welches Backup gerade läuft | Loading-Spinner auf dem "Run Now" Button + Real-time Progress via WebSocket (existiert schon teilweise) | Mittel |
| B4 | Stats oben (Active Targets, Total Backups etc.) könnten klickbar sein und als Filter dienen | Klick auf "3 Failed" → filtert nach Status "failed". Klick auf "Active Targets" → filtert nach enabled | Niedrig |
| B5 | Die Type-Filter Buttons (container, volume, stack, path) haben kein visuelles Feedback über die Anzahl | Badge/Count auf den Filter-Buttons: "Container (5)", "Stack (2)" | Niedrig |

### 3.3 Schedules-Seite

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| SC1 | Schedule-Cards zeigen nicht welche Targets sie nutzen | Expandierbarer Bereich oder Tooltip: "Used by: Target A, Target B" | Hoch |
| SC2 | Cron-Help nimmt 1/3 der Breite ein, auch wenn man es nicht braucht | Cron-Help als collapsible Panel oder als Popover statt permanente Sidebar. Spart Platz für die Schedule-Cards | Mittel |
| SC3 | "New Schedule" Form und Schedule-Cards sind im gleichen Bereich – wenn das Form offen ist verdrängt es die Cards | Form als Modal/Dialog öffnen statt inline, oder dedizierter Bereich oben | Niedrig |
| SC4 | Kein visueller Unterschied zwischen aktiven und inaktiven Schedules | Inaktive Schedules visuell gedimmt (opacity-50), am Ende der Liste, mit klarer "Inactive" Kennzeichnung | Niedrig |

### 3.4 Retention-Seite

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| RE1 | Delete-Button auf Policy hat kein ConfirmDialog (nur die Retention-Seite selbst, Wizard hat es) | ConfirmDialog implementieren wie bei Schedules und Storage | Hoch |
| RE2 | Policies zeigen nicht welche Targets sie nutzen | "Used by X targets" Badge. Bei 0: "Not in use" mit Option zum Löschen | Hoch |
| RE3 | Die GFS-Erklärungsbox nimmt viel Platz ein und ist statisch | Collapsible machen, standardmäßig zugeklappt wenn bereits Policies existieren | Niedrig |
| RE4 | Inline-Editing der Policy ist umständlich – alle Felder editieren sich gleichzeitig | Modal-basiertes Editing wie bei Schedules (Form in Dialog öffnen) für konsistentes Pattern | Mittel |
| RE5 | "Cleanup" Button oben rechts ist nicht selbsterklärend | Tooltip oder Sublabel: "Remove backup files that are no longer referenced" | Niedrig |
| RE6 | `keep_last` Feld fehlt in der PolicyCard und im CreatePolicyForm (nur im Wizard vorhanden) | `keep_last` Feld zur Retention-Seite hinzufügen – Konsistenz mit Wizard | Hoch |

### 3.5 Remote Storage-Seite

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| ST1 | Das Formular ist sehr lang (viele Storage-Type-spezifische Felder) – User muss scrollen | Formular in Steps aufteilen: (1) Typ wählen (2) Verbindungsdetails (3) Test & Save. Oder: Conditional Fields, die nur relevante Felder zum gewählten Typ zeigen | Mittel |
| ST2 | Storage-Type Buttons im Formular (Local, SSH, S3, etc.) sind nicht sehr intuitiv | Größere Cards mit Icon + Beschreibung (wie im Wizard Target-Type), Grid Layout 3x2 | Niedrig |
| ST3 | "Test Connection" Ergebnis verschwindet nach kurzer Zeit oder ist nicht im Kontext sichtbar | Test-Result persistent auf der Storage-Card anzeigen: Letzter Test: ✅ vor 2 Tagen / ❌ Connection refused | Mittel |
| ST4 | Browse-Funktion geht auf in einem overlay – man kann den Storage nicht gleichzeitig bearbeiten | Browse als separater Modus oder Drawer von rechts statt Modal-Overlay | Niedrig |

### 3.6 Settings-Seite

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| SE1 | "System Information" zeigt nur statische Pfade – wenig nützlich | Dynamische Infos: Disk Space, DB Size, Backup Count, Application Version, Uptime | Mittel |
| SE2 | Encryption Setup wird nicht erklärt – was passiert wenn man das aktiviert | Erklärungstext: "Backups will be encrypted before storage. You must keep your private key safe, or backups cannot be restored." mit Warnung | Mittel |
| SE3 | Komodo-Settings Edit-Mode: Man klickt "Edit" aber die Felder sind leer statt vorausgefüllt | ✅ Bereits gefixt (`initialFormData` aus Query) – aber nur wenn `handleStartEdit` aufgerufen wird. Der `formData` State wird mit leeren Werten initialisiert und erst beim Edit-Klick überschrieben. Potential für Race-Condition wenn Query noch lädt | Niedrig |

### 3.7 Login & SetupWizard

| # | Problem | Lösung | Priorität |
|---|---------|--------|-----------|
| LO1 | Nicht analysiert da nicht im Scope der aktuellen Probleme | ggf. separat analysieren | - |

---

## 4. Globale UX-Inkonsistenzen

### 4.1 Pattern-Inkonsistenzen

| # | Inkonsistenz | Wo | Fix |
|---|-------------|-----|-----|
| G1 | Delete-Bestätigung: `window.confirm()` vs `ConfirmDialog` | Retention-Seite nutzt noch direkte Mutation ohne Confirm | Überall ConfirmDialog verwenden |
| G2 | Edit-Pattern: Inline-Editing (Retention) vs Modal (Wizard) vs Form (Schedules/Storage) | Retention, Schedules, Storage haben alle unterschiedliche Edit-Patterns | Einheitlich: Form-Dialog als Modal für Edit-Operationen bei allen Entitäten |
| G3 | Empty-States: Unterschiedliche Größen und Styles | Backups-Empty ist groß (p-12), andere sind klein | Einheitliche Empty-State Komponente mit konfigurierbare Größe |
| G4 | Loading-States: Skeleton-Loader vs Spinner | Backups nutzt Skeleton, Schedules nutzt Skeleton, andere nutzen Spinner | Einheitlich Skeleton-Loader für Listen, Spinner nur für Actions |
| G5 | Sidebar zeigt "Backup Manager" statt "DockerVault" | Layout.tsx Sidebar-Header | Branding auf "DockerVault" anpassen |
| G6 | Kein Breadcrumb oder Kontext-Navigation | Überall | Optional: Breadcrumb unter dem Header für tiefere Navigation |

### 4.2 Fehlende Globale Features

| # | Feature | Beschreibung | Priorität |
|---|---------|-------------|-----------|
| GF1 | Toast-Notifications sind gut, aber es gibt keine persistenten Benachrichtigungen | Notification-Center: Icon in der Sidebar mit Badge für ungelesene Events (Backup failed, Storage connection lost) | Mittel |
| GF2 | Keine Keyboard-Shortcuts außerhalb des Wizards | Global Shortcuts: `n` für New Backup, `/` für Search, `?` für Help | Niedrig |
| GF3 | Kein Dark/Light Mode Toggle | Settings-Seite: Theme-Auswahl. Aktuell hardcoded dark-mode | Niedrig |
| GF4 | Keine Onboarding/Tutorial-Hinweise | First-Time Hints: "Create your first backup target" Tooltips bei leerer Anwendung | Niedrig |

---

## 5. Empfohlene User-Flows

### 5.1 Flow: Erster Backup einrichten (Happy Path)

```
Dashboard
  → Sieht "0 targets configured" + prominent "Set Up First Backup" CTA
  → Klickt CTA
  → Wizard öffnet sich

Step 1: Target
  → Wählt "Stack" (große Card-Buttons)
  → Sucht "my-app" in der SearchableList
  → Name wird auto-generiert: "Stack: my-app Backup"
  → Klickt "Next"

Step 2: Volumes (bei Stacks automatisch gezeigt)
  → Sieht alle 4 Volumes des Stacks
  → "Backup all volumes" ist default
  → Für "db-data" Volume: Per-Volume Rule → Exclude "*.log"
  → Klickt "Next"

Step 3: Dependencies
  → Stack-Container sind automatisch vorausgewählt
  → Dependency-Reihenfolge automatisch aus docker-compose erkannt
  → "Stop containers during backup" ist default an
  → Klickt "Next"

Step 4: Schedule
  → Keine Schedules vorhanden → "Create new" ist automatisch aktiv
  → Presets direkt sichtbar → Klickt "Daily at 3 AM"
  → Name wird vorausgefüllt: "Daily at 3 AM"
  → Vorschau: "Next runs: 23.02.2026 03:00, 24.02.2026 03:00, 25.02.2026 03:00"
  → Klickt "Next"

Step 5: Storage
  → Kein Remote Storage → kleiner Hinweis "Local only"
  → Step ist optional → Klickt "Next"

Step 6: Retention
  → Keine Policies → "Create new" automatisch aktiv
  → Klickt Preset "7 daily + 4 weekly"
  → Name vorausgefüllt: "7 daily + 4 weekly"
  → Vorschau: "~11 backups will be kept at any time"
  → Klickt "Next"

Step 7: Options
  → Gzip ist default → passt
  → Enabled ist default → passt
  → Klickt "Next"

Step 8: Summary
  → Übersicht aller Einstellungen
  → Jede Section hat "Edit" Pencil-Button
  → Klickt "Create Target"
  → Success Toast + Wizard schließt
  → Targets-Seite zeigt neues Target
```

### 5.2 Flow: Bestehendes Backup bearbeiten

```
Backups-Seite
  → Target-Card zeigt: Name, Typ, Letztes Backup, Schedule-Name
  → Klickt "Edit" (Pencil-Icon)
  → Wizard öffnet im Edit-Modus
  → ALLE Steps sind als "visited" markiert (grüne Checks)
  → User kann direkt zu jedem Step springen
  → Ändert z.B. den Schedule
  → Spring direkt zu Summary
  → Klickt "Save Changes"
```

### 5.3 Flow: Backup von der Backups-Seite starten

```
Backups-Seite
  → Findet Target via Suche oder Type-Filter
  → Sieht auf der eingeklappten Card:
    - Target-Name und Typ
    - Letztes Backup: "2h ago - ✅ 54 MB"
    - Schedule: "Daily at 3 AM"
    - Quick Actions: "Run Now" | "Edit" | "Expand"
  → Klickt "Run Now"
  → Button zeigt Spinner
  → WebSocket-Update zeigt Progress
  → Bei Abschluss: Toast + Card aktualisiert sich
```

---

## 6. Priorisierte Umsetzungs-Roadmap

### Phase 1: Kritische UX-Fixes (1-2 Tage)

Direkt benutzerseitig spürbare Verbesserungen:

1. **[T1]** Wizard: Preselection von Target-Typ und Target wenn vom Kontext gestartet
2. **[DA1]** Dashboard: Actionable Insights ("5/12 containers backed up")
3. **[DA4]** Dashboard: Alert-Banner bei Problemen
4. **[B1]** Backups-Seite: Last-Backup-Status auf eingeklappter Card
5. **[RE1]** Retention: ConfirmDialog für Delete
6. **[RE2]** Retention: "Used by X targets" anzeigen
7. **[RE6]** Retention: `keep_last` Feld auf Retention-Seite hinzufügen
8. **[SC1]** Schedules: Welche Targets nutzen diesen Schedule

### Phase 2: Wizard-Optimierungen (2-3 Tage) ✅ Done

Verbesserter Wizard-Flow:

9. **[V1]** ✅ Volume Config: Per-Volume-Rules als primäre Interaktion bei Stacks
10. **[D1]** ✅ Dependencies: Stack-Container vs. External-Container trennen
11. **[S2]** ✅ Schedule: Human-readable Cron-Vorschau
12. **[RS1]** ✅ Storage: Link öffnet in neuem Tab statt Wizard zu verlassen
13. **[SU1]** ✅ Summary: Edit-Buttons pro Section
14. **[SU2]** ✅ Summary: Volume-Config Section hinzufügen
15. **[W3]** ✅ Wizard: Step-Labels unter den Progress-Bubbles
16. **[W5]** ✅ Wizard: Edit-Modus alle Steps als visited
17. **[S4]** ✅ Schedule: Presets direkt sichtbar statt hinter Toggle

### Phase 3: Konsistenz & Polish (2-3 Tage)

Pattern-Vereinheitlichung:

18. **[G1]** ConfirmDialog überall einsetzen
19. **[G2]** Edit-Pattern vereinheitlichen (Modal-basiert)
20. **[G5]** Branding: "DockerVault" statt "Backup Manager"
21. **[D3]** Dependencies: "Select all" Logik für Stacks
22. **[B2]** Backups-Seite: Sortierung
23. **[B5]** Filter-Buttons mit Count-Badges
24. **[R1]** Retention: Visuelle Vorschau
25. **[SE1]** Settings: Dynamische System-Infos
26. **[SC2]** Schedules: Cron-Help als Popover

### Phase 4: Nice-to-Have (ongoing)

27. **[W2]** Wizard: LocalStorage Draft-Persistierung
28. **[T3]** Target-Select: "Already configured" Badge
29. **[DA5]** Dashboard: Disk-Usage Anzeige
30. **[V4]** Volume-Config: Volume-Größe anzeigen
31. **[GF1]** Notification-Center
32. **[D4]** Dependencies: Drag & Drop Reihenfolge
33. **[ST1]** Storage: Formular in Steps aufteilen

---

## 7. Design-Tokens & Patterns

### 7.1 Einheitliche Komponenten-Patterns

#### Selection Pattern (für Schedule, Retention, Storage im Wizard)
```
1. Wenn 0 Items: "Create new" automatisch aktiv, Form direkt zeigen
2. Wenn 1 Item: Item automatisch ausgewählt (highlighted), "Create new" als sekundäre Option
3. Wenn 2+ Items: Keins ausgewählt, User muss wählen. Items sortiert nach Nutzung
4. "None" Option: Immer am Ende, visuell dezent, mit Warnung
```

#### Edit Pattern
```
1. User klickt "Edit" auf einer Card/Liste
2. Modal/Dialog öffnet sich mit vorausgefülltem Formular
3. Formular hat: Save | Cancel
4. Save → Toast → Modal schließt → Liste aktualisiert
5. Cancel → Modal schließt ohne Änderung
```

#### Delete Pattern
```
1. User klickt "Delete" (Trash-Icon, red)
2. ConfirmDialog öffnet: "Delete {entity} '{name}'?"
3. Message: Erklärung der Konsequenzen
4. Confirm → Toast → Liste aktualisiert
5. Cancel → Dialog schließt
```

#### Empty-State Pattern
```
1. Icon (passend zum Kontext, muted)
2. Headline: "No {entities} configured"
3. Subtitle: Erklärung was diese Entität tut
4. CTA Button: "Create First {Entity}" (primary)
```

### 7.2 Wizard Smart-Default Logik

Für wiederverwendbare Entitäten (Schedule, Retention, Storage) im Wizard:

```typescript
function useSmartDefault<T extends { id: number }>(
  items: T[],
  isLoading: boolean,
  currentSelection: number | null,
  callbacks: {
    onAutoSelect: (item: T) => void
    onCreateNew: () => void
  }
) {
  // Bereits ausgewählt → nichts tun
  // 0 Items → "Create new" aktivieren
  // 1 Item → Item auto-selecten
  // 2+ Items → Neutral, User muss wählen
}
```

### 7.3 Informationshierarchie auf Cards

**Eingeklappte Card (Primary Info):**
```
[Icon] [Name]                                    [Status Badge] [Quick Actions]
       [Subtitle: Type + Target]                 [Last Backup: "2h ago ✅"]
```

**Aufgeklappte Card (Detail Info):**
```
[... eingeklappte Info ...]
---
[Schedule Info] [Retention Info] [Storage Info]
---
[Backup History – letzte 10]
  [Backup Row: Status | Date | Size | Duration | Actions]
```

---

## Anhang: Zusammenfassung aller 50+ Issues

| ID | Bereich | Titel | Priorität | Status |
|----|---------|-------|-----------|--------|
| T1 | Wizard/Target | Preselection beim Starten | Hoch | ✅ Done |
| T2 | Wizard/Target | Intelligentere Namen | Niedrig | Offen |
| T3 | Wizard/Target | "Already configured" Badge | Niedrig | Offen |
| T4 | Wizard/Target | Stack-Details expandierbar | Niedrig | Offen |
| V1 | Wizard/Volumes | Per-Volume prominent bei Stacks | Hoch | ✅ Done |
| V2 | Wizard/Volumes | Klareres "All vs. Specific" | Mittel | Offen |
| V3 | Wizard/Volumes | Volume-Labels/Container anzeigen | Mittel | Offen |
| V4 | Wizard/Volumes | Volume-Größe anzeigen | Niedrig | Offen |
| D1 | Wizard/Deps | Stack vs. External Container | Hoch | ✅ Done |
| D2 | Wizard/Deps | Auto-detect bei Stacks automatisch | Mittel | Offen |
| D3 | Wizard/Deps | "Select all" Logik für Stacks | Mittel | Offen |
| D4 | Wizard/Deps | Drag & Drop Reihenfolge | Niedrig | Offen |
| S1 | Wizard/Schedule | Nutzungs-Sortierung | Mittel | Offen |
| S2 | Wizard/Schedule | Human-readable Cron | Hoch | ✅ Done |
| S3 | Wizard/Schedule | "None" weniger prominent | Niedrig | Offen |
| S4 | Wizard/Schedule | Presets direkt sichtbar | Mittel | ✅ Done |
| RS1 | Wizard/Storage | Link öffnet neuen Tab | Hoch | ✅ Done |
| RS2 | Wizard/Storage | Kompakterer Empty-State | Niedrig | Offen |
| RS3 | Wizard/Storage | Multiselect Hinweis | Niedrig | Offen |
| R1 | Wizard/Retention | Visuelle Vorschau | Mittel | Offen |
| R2 | Wizard/Retention | Preset füllt Namen | Niedrig | Offen |
| R3 | Wizard/Retention | Konkretere "None" Warnung | Niedrig | Offen |
| O1 | Wizard/Options | Compression hauptsächlich | Niedrig | Offen |
| O2 | Wizard/Options | Compression-Erklärung | Niedrig | Offen |
| SU1 | Wizard/Summary | Edit-Buttons pro Section | Mittel | ✅ Done |
| SU2 | Wizard/Summary | Volume-Config Section | Mittel | ✅ Done |
| W1 | Wizard/Global | Larger modal Option | Niedrig | Offen |
| W2 | Wizard/Global | LocalStorage Draft | Mittel | Offen |
| W3 | Wizard/Global | Step-Labels | Mittel | ✅ Done |
| W4 | Wizard/Global | Skipped Steps kleiner | Niedrig | Offen |
| W5 | Wizard/Global | Edit-Modus visited Steps | Mittel | ✅ Done |
| DA1 | Dashboard | Actionable Insights | Hoch | ✅ Done |
| DA2 | Dashboard | Quick-Actions | Mittel | Offen |
| DA3 | Dashboard | "View All" Link | Niedrig | Offen |
| DA4 | Dashboard | Alert-Banner | Hoch | ✅ Done |
| DA5 | Dashboard | Disk-Usage | Mittel | Offen |
| B1 | Backups | Last-Backup sichtbar | Hoch | ✅ Done |
| B2 | Backups | Sortierung | Mittel | Offen |
| B3 | Backups | Run-Now Feedback | Mittel | Offen |
| B4 | Backups | Klickbare Stats | Niedrig | Offen |
| B5 | Backups | Filter mit Count | Niedrig | Offen |
| SC1 | Schedules | Targets anzeigen | Hoch | ✅ Done |
| SC2 | Schedules | Cron-Help Popover | Mittel | Offen |
| SC3 | Schedules | Form als Modal | Niedrig | Offen |
| SC4 | Schedules | Inactive dimmen | Niedrig | Offen |
| RE1 | Retention | ConfirmDialog | Hoch | ✅ Done |
| RE2 | Retention | "Used by X" Badge | Hoch | ✅ Done |
| RE3 | Retention | GFS-Box collapsible | Niedrig | Offen |
| RE4 | Retention | Modal-basiertes Editing | Mittel | Offen |
| RE5 | Retention | Cleanup Tooltip | Niedrig | Offen |
| RE6 | Retention | keep_last Feld | Hoch | ✅ Done |
| ST1 | Storage | Form in Steps | Mittel | Offen |
| ST2 | Storage | Größere Type-Cards | Niedrig | Offen |
| ST3 | Storage | Persistent Test-Result | Mittel | Offen |
| ST4 | Storage | Browse als Drawer | Niedrig | Offen |
| SE1 | Settings | Dynamische Infos | Mittel | Offen |
| SE2 | Settings | Encryption Erklärung | Mittel | Offen |
| SE3 | Settings | Form Race-Condition | Niedrig | Offen |
| G1 | Global | ConfirmDialog überall | Mittel | Offen |
| G2 | Global | Edit-Pattern einheitlich | Mittel | Offen |
| G3 | Global | Empty-State Komponente | Niedrig | Offen |
| G4 | Global | Loading-State einheitlich | Niedrig | Offen |
| G5 | Global | Branding "DockerVault" | Niedrig | Offen |
| GF1 | Global | Notification-Center | Mittel | Offen |
| GF2 | Global | Keyboard-Shortcuts | Niedrig | Offen |
| GF3 | Global | Theme-Toggle | Niedrig | Offen |
| GF4 | Global | Onboarding-Hints | Niedrig | Offen |
