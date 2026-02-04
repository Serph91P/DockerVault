# DockerVault - Feature Tracking

## Aktueller Stand (04.02.2026)

### ✅ Erledigt
- [x] Basis-Anwendung läuft (FastAPI Backend + React Frontend)
- [x] Docker-Integration (Container, Volumes, Stacks auflisten)
- [x] Backup-Engine für Container, Volumes, Paths, Stacks
- [x] Retention Policies (global)
- [x] Remote Storage (S3, FTP, WebDAV)
- [x] Komodo-Integration (optional)
- [x] Komodo-Settings im Frontend editierbar
- [x] WebSocket für Echtzeit-Updates
- [x] ARM64 Build aus CI entfernt (schnellere Builds)
- [x] TypeScript/Python Linting Fehler behoben
- [x] Logging im Backend konfiguriert
- [x] **Schedules neu designen** - Schedule als eigenständige Entität (04.02.2026)
- [x] Backend: Neues `Schedule` Model in database.py
- [x] Backend: Schedule API (CRUD für Schedules)
- [x] Backend: Target API anpassen (schedule_id)
- [x] Backend: Scheduler für neues Modell angepasst
- [x] Frontend: Schedules-Seite zum Erstellen/Bearbeiten
- [x] Frontend: Target-Formular mit Schedule-Dropdown
- [x] **Retention Policy pro Target** - Target-spezifische Retention (04.02.2026)

### 🚧 In Arbeit / Geplant

#### 1. ~~Schedules neu designen~~ ✅ ERLEDIGT (04.02.2026)
**Implementiert:**
- Neues `Schedule` Model als eigenständige Entität
- CRUD API für Schedules (`/api/v1/schedules`)
- Targets referenzieren Schedules via `schedule_id`
- Backwards-kompatibel: `schedule_cron` weiterhin unterstützt
- Frontend: Schedules erstellen/bearbeiten/löschen
- Frontend: Schedule-Dropdown in Target-Cards

---

#### 2. ~~Setup-Wizard für neue Backups~~ ✅ ERLEDIGT (04.02.2026)
**Implementiert:**
- Multi-Step Wizard-Komponente (`components/BackupWizard/`)
- 7 Steps: Target → Dependencies → Schedule → Storage → Retention → Options → Summary
- Step-by-Step Navigation mit Progress-Anzeige
- Schedule-Erstellung direkt im Wizard möglich
- Integration in Targets-Seite mit "New Target" Button
- Cron-Presets und Hilfe für Cron-Expressions

**Komponenten:**
- `BackupWizard/index.tsx` - Hauptkomponente mit State-Management
- `StepTargetSelect.tsx` - Target-Typ und Auswahl
- `StepDependencies.tsx` - Container-Abhängigkeiten
- `StepSchedule.tsx` - Schedule wählen/erstellen
- `StepStorage.tsx` - Remote Storage Auswahl
- `StepRetention.tsx` - Retention Policy
- `StepOptions.tsx` - Erweiterte Optionen
- `StepSummary.tsx` - Zusammenfassung vor Erstellung

**Noch zu ergänzen:**
- [ ] Backend: Dependency-Erkennung für Stacks
- [ ] Backend: Endpoint für Stack-Analyse (`/api/docker/stacks/{name}/dependencies`)
- [ ] Retention Policy inline erstellen (API fehlt noch)

---

#### 3. ~~Retention Policy pro Target~~ ✅ ERLEDIGT (04.02.2026)
**Implementiert:**
- Retention Policy kann pro Target überschrieben werden
- `BackupTarget.retention_policy_id` referenziert spezifische Policy
- Wenn NULL → globale Policy verwenden
- `keep_last` zu allen Retention-Modellen hinzugefügt
- `RetentionPolicyInfo` Embedded Model für Target-Responses
- Target API liefert jetzt Retention-Policy-Details mit
- Frontend: Retention-Badge auf Target-Cards
- Wizard: Retention-Policy Auswahl mit keep_last Anzeige

---

#### 4. Automatische Dependency-Erkennung bei Stacks (Mittlere Priorität)
**Anforderung:** Bei Stacks sollen Abhängigkeiten automatisch erkannt werden.

**Logik:**
```python
# Beim Stack-Backup:
1. docker-compose.yml parsen
2. depends_on extrahieren
3. Stopp-Reihenfolge berechnen (reverse topological sort)
4. Backup durchführen
5. Start-Reihenfolge berechnen (topological sort)
```

**Beispiel:**
```yaml
services:
  app:
    depends_on: [db, redis]
  db: {}
  redis: {}
```
→ Stopp: app → redis → db
→ Start: db → redis → app

**TODO:**
- [ ] Backend: Stack-Analyse Funktion
- [ ] Backend: Topological Sort für Abhängigkeiten
- [ ] Backend: Backup-Engine mit korrekter Reihenfolge
- [ ] Frontend: Abhängigkeiten im Wizard anzeigen

---

#### 5. Verbessertes Backup-Logging (Niedrige Priorität)
- [ ] Detaillierte Logs pro Backup-Job
- [ ] Logs im Frontend anzeigbar
- [ ] Fehler-Details bei fehlgeschlagenen Backups

---

#### 6. Backup-Restore Funktion (Zukünftig)
- [ ] Restore-Wizard
- [ ] Backup auswählen
- [ ] Ziel wählen (Original oder neuer Container/Volume)
- [ ] Restore-Vorschau
- [ ] Restore durchführen

---

## Datenbank-Änderungen

### Neue Tabelle: `schedules`
```sql
CREATE TABLE schedules (
    id INTEGER PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    cron_expression VARCHAR(100) NOT NULL,
    enabled BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### Änderung: `backup_targets`
```sql
-- ALT
schedule_cron VARCHAR(100)

-- NEU
schedule_id INTEGER REFERENCES schedules(id)
```

### Migration
```python
# 1. Neue schedules Tabelle erstellen
# 2. Für jeden einzigartigen schedule_cron einen Schedule erstellen
# 3. schedule_id in backup_targets setzen
# 4. schedule_cron Spalte entfernen
```

---

## Notizen

### Prioritäten für morgen:
1. **Schedule-Redesign** - Erst Backend, dann Frontend
2. **Wizard-Grundgerüst** - UI-Komponente erstellen
3. **Dependency-Erkennung** - Stack-Analyse implementieren

### Offene Fragen:
- Sollen gelöschte Schedules auch die Targets "entkoppeln" oder Fehler werfen?
- Wizard als Modal oder eigene Seite?
- Wie mit laufenden Backups umgehen wenn Schedule geändert wird?

---

## Git Branches
- `develop` - Aktueller Entwicklungsstand
- TODO: Feature-Branches für größere Änderungen?
  - `feature/schedule-redesign`
  - `feature/backup-wizard`
