import MapKit
import SdrmmCore
import SwiftUI

struct SurveyScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var survey = model.survey
        VStack(spacing: 0) {
            SurveyHeader()
                .padding(.horizontal)
                .padding(.vertical, 8)
            Map(position: $survey.camera) {
                UserAnnotation()
                ForEach(survey.runs) { run in
                    MapPolyline(coordinates: run.coordinates.map(\.coordinate))
                        .stroke(Palette.survey[run.bin], lineWidth: 4)
                }
            }
            .mapControls {
                MapCompass()
                MapScaleView()
            }
            .accessibilityIdentifier(A11y.surveyMap)
            SurveyActions(controls: model.openMission?.controls ?? [])
                .padding()
        }
    }
}

private struct SurveyHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let survey = model.survey
        let view = survey.view
        VStack(spacing: 6) {
            HStack {
                Text(view.map { FrequencyText.text($0.freqHz) } ?? "-")
                    .font(.title3.weight(.semibold).monospacedDigit())
                    .accessibilityIdentifier(A11y.surveyFreq)
                Spacer()
                Text(LevelText.db(view?.levelDb))
                    .font(.title3.monospacedDigit())
                    .accessibilityIdentifier(A11y.surveyLevel)
            }
            HStack(spacing: 6) {
                Text(LevelText.db(view?.minDb))
                LinearGradient(colors: Palette.survey, startPoint: .leading, endPoint: .trailing)
                    .frame(height: 8)
                    .clipShape(Capsule())
                    .accessibilityHidden(true)
                Text(LevelText.db(view?.maxDb))
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier(A11y.surveyLegend)
            if survey.trimmed {
                Chip(text: "Trail trimmed", color: Palette.warn)
                    .accessibilityHint("Oldest trail points were dropped")
                    .accessibilityIdentifier(A11y.surveyTrimmed)
            }
        }
    }
}

private struct SurveyActions: View {
    @Environment(AppModel.self) private var model
    let controls: [MissionControl]

    var body: some View {
        let survey = model.survey
        let recording = survey.view?.recording == true
        HStack(spacing: 12) {
            Button {
                Task { await survey.toggleRecording() }
            } label: {
                Label(recording ? "Stop" : "Record", systemImage: recording ? "stop.fill" : "record.circle")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .tint(recording ? Palette.danger : Palette.accent)
            .disabled(survey.busy || !controls.contains(.surveyRun))
            .accessibilityIdentifier(A11y.surveyRun)
            Button("Clear") { survey.clearTrail() }
                .buttonStyle(.bordered)
                .accessibilityHint("Clears the trail on this phone")
                .accessibilityIdentifier(A11y.surveyClear)
            Button("Fit") { survey.fit() }
                .buttonStyle(.bordered)
                .accessibilityIdentifier(A11y.surveyFit)
        }
    }
}
