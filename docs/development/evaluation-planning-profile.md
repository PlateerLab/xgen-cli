# 계획 크기 평가 profile

2026-10-05. 범용 구현. [pilot 연구와 측정 계약](../research/2026-10-05-planning-size-model-pilot-research.md)의 첫 구현 단위야. 실제 모델 pilot 결과는 아니야.

`OpenAiPlannerConfig::with_evaluation_proposal_step_limit`로 평가용 공통 prompt를 선택해. `EvaluationProposalStepLimit::One`은 X1, `Four`는 XN이야. X0는 이 builder를 호출하지 않은 기존 production config야.

```rust
use xgen_provider_openai::{EvaluationProposalStepLimit, OpenAiPlannerConfig};

let config = OpenAiPlannerConfig::new(
    "http://127.0.0.1:8000/v1", "xgen.eval", "fixture-model", "fixture-tokenizer",
)?
.with_evaluation_proposal_step_limit(EvaluationProposalStepLimit::Four)?;
```

이 API는 모델 pilot runner에서 명시적으로 사용할 수 있어. CLI 기본 선택이나 사용자 설정 옵션에는 연결하지 않았어. 모델·thinking·dialect·budget·도구 catalog는 runner에서 동일하게 고정해야 해. 이 builder 자체가 모든 실험 설정을 고정해 주는 것은 아니야.

## 계약

- X1/XN은 공통 prompt를 쓰고 `maxProposalSteps=1`/`4`, schema의 `steps.maxItems`, 같은 상한을 사용하는 local gate만 달라져. 최대 4개는 반드시 4개라는 뜻이 아니야.
- 각 Step의 arguments는 현재 context에서 이미 아는 concrete literal이어야 해. 관찰이 필요한 다음 argument는 Receipt-bound output을 받은 뒤 별도 turn에서 정해. Dependency는 순서를 표현하고 output substitution을 만들지 않아.
- 평가 profile의 plan은 1개 이상, 상한 이하 Step이어야 해. 초과·빈 plan은 `InvalidResponse`로 거절해. 자르거나 일부만 admission에 넘기지 않아.
- Completion/response candidate의 빈 `steps`는 유지해. 그래서 공통 schema에 `minItems=1`을 넣지 않아. Branch별 계약은 local decoder가 확인해.
- `json_schema`, `json_object`, 기존 atomic JSON codec에서 상한을 유지해. Dialect builder 전후 어느 순서로 설정해도 같은 commitment가 나와. Conversation response 기능은 기존 명시적 builder로만 켜지고 prompt/schema digest에 반영돼.
- Prompt/schema digest에 상한을 결합해. `planningConstraints`의 필수 여부도 prompt에 결합해 같은 prompt bytes로 서로 다른 context 계약을 숨기지 않아.
- Opt-in하지 않은 production prompt/schema/revision/digest는 유지해. 새로운 Core event나 resume format을 추가하지 않아.
- Adapter rejection도 기존 usage observer에 `ResponseRejected`와 provider-reported usage로 전달해. `ProposalDecoded`와 Core admission은 다른 단계야. Missing usage는 기존대로 UNKNOWN이며 비용 0을 의미하지 않아.

Concrete literal 여부 중 의미적 조건은 모델에게 주는 계약이야. 이 변경의 local gate는 Step 개수를 검사해. 임의 문자열의 의미를 판별하거나 미래 관찰의 provenance를 자동 증명하는 새 validator는 추가하지 않았어. Capability argument schema, dependency graph, material binding, 승인, Receipt는 기존 Core 경로를 사용해.

## 검증 범위

Provider unit test는 두 dialect에서 X1/XN request를 상한만 정규화한 뒤 전체 비교해. Conversation 확장에서도 같은 비교를 하고, constraints 유무에 따라 digest가 달라지는지 검사해. 세 dialect의 builder 순서도 확인해.

서로 다른 filesystem/process argument shape에 대해 0·1·상한·상한+1개를 decode해. 상한 이내는 개수를 보존하고 범위 밖은 거절하며, completion candidate의 빈 배열은 허용해. 이 unit fixture는 실제 filesystem/process 실행 결과가 아니야.

Loopback HTTP/Core 계약 테스트는 다음을 확인해.

- 1개와 4개 계획이 Core에 전체 채택되고 proposed dependency가 보존돼.
- 이미 ready work가 있으면 추가 모델 요청 없이 action을 반환해.
- 2개/X1, 5개/XN은 materialization 전에 거절하고 runnable Step을 만들지 않아.
- 거절된 응답도 usage/cache 관측과 model-call reserve/settle 1회가 남아.
- Receipt-completed output snapshot이 후속 요청에 정확히 전달되고 existing dependency를 포함한 계획이 materialization 단계에 도달해. 이 snapshot fixture는 materializer에서 의도적으로 중단하며 새 계획 commit이나 도구 실행을 주장하지 않아.

전체 workspace tests, clippy, fmt와 공개 문서 계약 검사로 기존 경로의 회귀를 확인해. Live endpoint smoke는 실행하지 않았어.

2026-10-05 검증 기록: `cargo test --workspace` 659 passed, 0 failed, 4 ignored. Ignored live tests는 실행하지 않았어. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, `scripts/check-rc3-public-docs.sh`, `git diff --check`도 PASS야.

## 남은 작업

2026-10-05 후속 구현: [실제 driver 기반 pilot runner](planning-model-pilot-runner.md)에 profile·usage·독립 oracle·전송 전 비용 예약을 연결했어. 아래 1번 구현의 계약 검사를 제공하며 실제 API smoke와 최종 120 trial은 아직이야.

1. 범용: 실제 driver, usage observer, 독립 oracle를 묶은 model pilot runner. Binary/config/fixture 고정, decode/admission 분리, unknown-call reserve와 지출 상한이 필요해.
2. 범용: Design fixture와 비용 조건을 고정한 작은 API smoke. Model alias/응답 ID, usage/cache, schema 호환성을 확인해.
3. 범용: Validation fixture와 manifest를 잠근 뒤 사전 정의한 120 trial을 실행하고 X1/XN 비용·완료·false claim을 비교해.

실제 모델 호출 수, 비용 절감, 정확도 개선은 모두 NOT_MEASURED야. 평가 profile이 구현됐다는 사실만으로 production 기본값을 변경하지 않아.

## 후속 본실험 결과

2026-10-05 [등록한 120회 pilot](planning-model-pilot-results-2026-10-05.md)을 완료했어. 작업 oracle 120/120, 응답 계약 포함 94/120이야. Validation XN 비용 median은 X1 대비 24.5% 낮았지만 응답 형식 위반 1회가 남아 gate는 FAIL이고 production 기본값을 유지해. 위 미실행 표시는 각 구현·등록 시점의 기록이야.
