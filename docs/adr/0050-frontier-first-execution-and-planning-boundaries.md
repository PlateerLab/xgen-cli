# ADR-0050: Frontier-first 실행 유지와 모델 판단 경계 평가

- 상태: Proposed — 비교 평가 전 초안
- 기준일: 2026-10-05
- 변경 분류: 범용 아키텍처. 사례별 engine 규칙 없음.
- 기존 경계: ADR-0009, ADR-0012, ADR-0014, ADR-0016, ADR-0019, ADR-0021
- protocol·journal·SQLite schema 변경: 없음
- runtime 구현 변경: 없음
- 근거: [실행 책임 비교](../research/2026-10-05-harness-architecture-comparison.md)
- 검증 gate: [계획 경계 평가](../research/2026-10-05-planning-boundary-evaluation.md)

## 문제

별도 하네스 엔진의 가치를 입증하려면 다른 loop와 다른 구조라는 설명을 넘어, 책임 경계가 정확도·비용·복구에 어떤 효과를 주는지 검증해야 한다.

현재 xgen은 이미 다음 구조다.

```text
verified current state
  → derived frontier의 기존 작업 처리
  → 작업이 없고 복구·차단 조건도 없을 때 model-call reservation
  → 비신뢰 proposal 검증·원자 저장
  → admission·실행·검증·Receipt
```

`AgentLoop::tick`은 action을 선택하고 `RunDriver`가 trusted ports를 연결한다. WorkGraph가 단순 표시용 plan인 것은 아니다. 따라서 “계획/실행 분리 도입”을 새로운 리팩터링 목표로 삼으면 이미 있는 계약을 중복 구현할 수 있다.

그러나 DAG의 존재만으로 결과 전달·병렬 실행·실패한 graph의 자동 재계획을 보장하지 않는다. 현재 계획의 arguments는 commit 전에 구체화되고, 순서 dependency는 typed output→input reference와 다르다. 이를 하나의 기능처럼 약속하면 승인의 concrete resource와 durable material 의미가 불명확해진다.

## 제안

### 1. 기존 실행 권위를 유지한다

한 Run에 하나의 orchestration authority를 유지한다. Core는 invocation과 실행 사실의 결합, dependency release, Receipt terminalization을 소유한다. Planner는 계획·응답 후보를 제안하며 실행 권한이나 성공 사실을 확정하지 않는다. UI는 같은 driver를 사용한다.

기존 frontier action은 새 모델 호출보다 우선한다. Proposal commit은 tool 실행 승인과 구분하며 각 concrete invocation의 admission을 유지한다. Unknown effect를 “계획 변경”으로 우회하지 않는다.

실패·manual 상태의 descendants 차단과 독립 branch 처리를 구분한다. 실행 가능한 독립 action이 남아 있으면 이를 먼저 처리할 수 있다. Multi-step proposal에 전체 transaction rollback이나 첫 실패 후 모든 sibling 중단 의미를 추가하지 않는다.

### 2. 짧은 계획과 관찰 후 재계획을 함께 평가한다

전체 목표의 계획을 첫 turn에 확정하도록 강제하지 않는다. 이미 알고 있는 입력으로 실행 가능한 literal 작업은 묶을 수 있고, 결과 해석이나 다음 resource 선택이 필요하면 관찰 뒤 모델을 호출한다. 목표는 모델 호출 수 최소화 자체가 아니라, 정확한 완료와 복구를 유지하면서 호출·token·시간을 줄이는지 확인하는 것이다.

첫 실험은 같은 xgen runtime에서 한 Step씩 계획하는 조건과 여러 literal Step을 계획하는 조건을 비교한다. 같은 도구·승인·budget으로 계획 크기만 바꾼다. 기존 엔진/SDK의 비교는 버전과 실행 환경을 고정한 별도 단계다.

### 3. 결과를 다음 입력에 연결하는 계약은 별도 gate로 둔다

이 ADR은 output reference 구현을 승인하거나 wire syntax를 정의하지 않는다. 평가에서 단순 값 전달 때문에 모델 왕복이 반복되는 것이 확인될 때 다음 ADR을 작성한다.

후속 계약이 필요하면 최소한 다음을 명시해야 한다.

- Receipt-bound source Step·output identity·digest와 bounded JSON Pointer.
- destination input schema, 허용 타입·크기와 reference cycle 거부.
- source 값이 확정된 뒤 argument materialization, concrete resource resolution, permission request와 intent commit을 수행하는 순서.
- source 변경·손실·schema drift·crash 후 binding 재검증 및 기존 store migration.
- 모델이 지어낸 output이나 임의 문자열 치환으로 resource·권한·도구를 만들 수 없는 경계.

Engine에 범용 expression language나 모델 출력의 임의 코드 실행을 추가하는 것으로 문제를 해결하지 않는다. 실제 새 입력을 만들거나 해석하는 계산은 명시적인 Capability 계약으로 표현할 수 있다.

### 4. 병렬 실행과 외부 workflow 엔진 교체는 분리한다

Read-only 작업도 공통 resource·snapshot·rate limit을 공유할 수 있다. Parallel scheduler는 conflict·lease·budget·cancel·unknown·Receipt commit 규칙을 별도로 다뤄야 한다. 첫 비교에서 직렬 batching의 효과와 병렬화 효과를 섞지 않는다.

Temporal/Restate는 원격 worker·durable timer·장기 서버 작업의 후보로 남긴다. 로컬 기본 경로에 서버를 추가하거나 같은 Parent Run을 두 엔진이 함께 소유하지 않는다. 필요 시 상위 엔진과 bounded child Run의 authority·idempotency·Receipt 경계를 별도 ADR로 정한다.

## 대안

| 범용 대안 | 판단 |
|---|---|
| 기존 frontier-first 유지 | 기본 경로. 이미 구현된 계약을 확인하고 측정한다. |
| 전체 계획 선확정 | 관찰이 필요한 탐색 작업에서의 효용을 입증하지 못했으므로 기본값으로 채택하지 않는다. |
| 즉시 typed dataflow·parallel scheduler 구현 | admission/materialization 의미 변경과 병렬 효과가 섞여 원인을 구분하기 어렵다. gate 이후 분리한다. |
| 기존 workflow 엔진으로 전면 교체 | 배포·authority·Receipt 경계를 비교하기 전에는 선택하지 않는다. |
| 외부 SDK를 감싸 같은 목표를 다시 계획 | ADR-0009의 단일 authority와 충돌할 수 있다. 관찰 또는 bounded child 실행으로 책임을 구분해야 한다. |

## 예상 결과와 반증

기존 CLI의 thinking 상태·최종 응답 UX는 유지할 수 있다. 내부 DAG나 Receipt를 사용자에게 항상 노출할 필요는 없다. 사용자 목표의 정확한 완료 여부는 별도 oracle로 평가한다.

현재 구조가 우월하다는 주장은 보류한다. 여러 literal 작업을 묶어도 비용·시간이 줄지 않거나 held-out 탐색 작업의 정확도·안전성이 나빠지면 기본 planner policy를 바꾸지 않는다. 안전하게 중단한 비율과 사용자 작업 완료율을 따로 보고한다.

모델·policy·catalog 교체, graph replacement, 자동 실패 복구, memory injection은 이번 제안에 포함하지 않는다. 현재 manifest/profile 결합과 immutable dependency 계약이 우선한다.

## 초안 채택 조건

1. 소스 기준 frontier-first·multi-step plan·Receipt dependency가 기존 regression에서 확인된다.
2. 계획 크기만 다른 조건을 최소 두 작업 유형과 분리된 validation 입력에서 비교한다.
3. 정상 실행과 fault injection을 구분하고 실패·중단·Unknown을 제외하지 않는다.
4. 결과를 확인하기 전에 승인·budget·metric·반복 수·채택 기준을 고정한다.
5. Candidate마다 비용·latency·완료 정확도·manual intervention과 미해결 상태를 보고한다.

문서 병합은 연구 방향과 평가 계약의 기록이다. 평가를 통과한 정책이나 새로운 실행 보장이 출시됐다는 뜻은 아니다.
