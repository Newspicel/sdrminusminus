package dev.newspicel.sdrmm.survey

import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.map.SurveyTrail
import dev.newspicel.sdrmm.map.TrailRun
import dev.newspicel.sdrmm.mission.MissionRunner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

class SurveyTrailStore(
    private val core: CoreGateway,
    private val runner: MissionRunner,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private val trail = SurveyTrail()
    private val lock = Mutex()
    private val runsState = MutableStateFlow<List<TrailRun>>(emptyList())
    private val trimmedState = MutableStateFlow(false)
    val runs: StateFlow<List<TrailRun>> = runsState.asStateFlow()
    val trimmed: StateFlow<Boolean> = trimmedState.asStateFlow()

    fun run(scope: CoroutineScope): Job = scope.launch {
        launch {
            core.surveyPoints.collect { points ->
                val view = core.survey.value
                lock.withLock {
                    trail.extend(points, view?.minDb ?: Float.NaN, view?.maxDb ?: Float.NaN, clock())
                    publish()
                }
            }
        }
        launch {
            var total = core.survey.value?.total ?: 0uL
            core.survey.collect { view ->
                val now = view?.total ?: return@collect
                if (now == 0uL && total > 0uL) clear()
                total = now
            }
        }
        launch {
            var last = runner.open.value
            runner.open.collect { open ->
                if (open != null && open != last) clear()
                last = open ?: last
            }
        }
    }

    suspend fun clear() {
        lock.withLock {
            trail.clear()
            publish()
        }
    }

    private fun publish() {
        runsState.value = trail.runs
        trimmedState.value = trail.trimmed
    }
}
