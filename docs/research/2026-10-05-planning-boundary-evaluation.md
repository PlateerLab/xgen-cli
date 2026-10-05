# 계획 크기·모델 판단 경계 비교 평가

- 기준일: 2026-10-05
- 상태: 사전등록 초안. Benchmark fixture·runner·외부 엔진 adapter는 아직 구현하지 않았다.
- 변경 분류: 범용 평가. 아래 사례는 평가용이며 engine/prompt의 사례별 조건문으로 옮기지 않는다.
- 근거: [아키텍처 비교](2026-10-05-harness-architecture-comparison.md), [ADR-0050](../adr/0050-frontier-first-execution-and-planning-boundaries.md)
- 상위 평가 계약: [기존 runtime 평가 프로토콜](2026-08-28-runtime-evaluation-protocol.md)

## 평가 질문

1. 현재 frontier-first 엔진에서 여러 literal Step을 한 계획으로 제안하면 모델 왕복·token·latency가 감소하는가?
2. 관찰 뒤 다음 입력을 결정하는 작업에서 batching이 잘못된 선확정이나 완료 보고를 늘리는가?
3. 정상 완료 개선과 안전한 장애 중단을 동시에 유지하는가? 복구 노력을 사용자에게 전가하지 않는가?

모델/실행 분리가 이미 있는 상태에서 새 구조의 우위를 가정하지 않는다. 먼저 계획 크기의 효과를 분리한다.

## 비교 조건

| 조건 | 변경 변수 | 고정 요소 | 상태 |
|---|---|---|---|
| X1 | proposal마다 최대 1 Step | 같은 xgen Core·도구·승인·provider·budget | 평가용 planner wrapper 필요 |
| XN | proposal마다 최대 4 literal Step, 값이 미확정이면 관찰 후 계획 | X1과 같음. 실행은 직렬 | 평가용 planner wrapper 필요 |
| X0 | 현재 production planner 설정 그대로 | 동일 environment와 case | 현실성 확인용 reference |
| XS | deterministic scripted planner가 정답 계획을 제안 | 같은 admission·adapter·verifier | engine 동작과 호출 수 계측 확인용, 성능 비교 제외 |

X1/XN은 공통 planning contract에 `maxProposalSteps` 제한만 다르게 전달하고 평가 runner의 wrapper가 이를 검사한다. 모델에게 제한을 알리지 않은 채 큰 proposal만 거절하는 비교를 만들지 않는다. Domain 표현, 성공 답안, case ID, oracle 정보는 모델 prompt에 넣지 않는다. Reject된 proposal을 조용히 잘라 실행하지 않으며 원본과 rejection을 기록한다. 각 조건의 request profile·config는 별도로 고정한다. X0를 XN과 같다고 가정하지 않고 실제 제안 크기를 측정한다.

XS는 모델 추론·정확도 우위를 평가하지 않는다. 예를 들어 literal 독립 작업 N개와 최종 응답에 대해 X1 방식은 N+1회, 한 packet에 담는 XS 방식은 2회의 planner 호출이 가능함을 계측하는 계약 probe다. 실제 모델에서 그 수가 나온다는 주장이 아니다.

## 사례와 분리

| split | case family | 관찰 경계 | 독립 oracle |
|---|---|---|---|
| design D1 | 고정 경로의 두 입력 읽기와 bounded 결과 생성 | 입력 경로는 요청에 명시. 작업을 묶을 수 있음 | 결과 파일의 구조·내용·불필요 변경 검사 |
| design D2 | 작은 Rust project의 failing test 진단·수정 | test 결과와 source를 읽은 뒤 수정 내용을 결정 | 모델이 보지 못한 regression test, diff 범위 검사 |
| validation V1 | 서로 다른 구조의 manifest를 읽고 artifact 목록 구성 | 요청의 literal 경로와 읽은 데이터의 후속 값 선택 구분 | 사전 고정된 typed 결과와 파일 상태 비교 |
| validation V2 | 작은 Python CLI project의 실패 진단·수정 | process 결과에 따라 읽을 파일·수정·검증 선택 | 별도 regression test, exit status와 artifact 검사 |

이는 family 정의이며 validation의 실제 repository contents·값·정답은 아직 정하지 않았다. 설계에 사용한 파일을 이름만 바꿔 holdout으로 쓰지 않는다. 별도 fixture 작성 단계에서 최소 4개 design instance와 4개 validation instance를 만들고 hash manifest를 고정한다. Validation instance의 숨겨진 test와 oracle은 모델·planner wrapper·설계용 trial에서 제외한다. 이후 validation 결과로 수정하면 새 미검토 validation set에서 다시 평가한다.

파일명·데이터 값·실패 위치·dependency 구조가 달라야 하며, 동일 정답의 번역은 다른 사례로 세지 않는다. Public reporting에는 fixture hash와 일반 지표만 포함한다.

## 실행 단계

### 0. 기존 계약 확인

`durable_agent_loop` 34개와 `durable_driver` 2개는 조사 중 통과했다. 두 suite는 frontier 우선, atomic plan, SQLite reopen과 실제 adapter 경로를 확인한다. 이 결과를 새 비교 benchmark 실행 건수에 합산하지 않는다.

### 1. 모델 없는 계약 probe

XS로 literal multi-step plan과 output-dependent plan 추가를 실행한다. 동일 sink에 요청 ID·attempt·applied 상태를 따로 남긴다. 각 approved effect와 Receipt를 연결하고 복구 뒤 effect count를 확인한다.

정상 경로와 아래 fault 위치 각각을 fixture당 10회 반복한다. 디자인과 validation 각각 최소 2개 서로 다른 fixture를 사용한다. `validation` label은 engine probe에서도 별도로 유지한다.

| fault | 검증할 계약 | 기대 분류 |
|---|---|---|
| plan commit 후 첫 admission 전 process exit | 저장 계획 복원, 새 모델 호출 없이 기존 frontier 선택 | 재개 가능 |
| sink applied 후 outcome commit 전 process exit | 동일 effect를 blind retry하지 않음 | reconciliation 가능하면 증거로 복구, 아니면 unresolved/수동 복구 |
| EffectSucceeded 후 Receipt transaction 전 process exit | effect 재실행 없이 verifier 재개 | 검증 후 확정 또는 검증 불확실 |
| model reservation 후 response settlement 전 process exit | 암묵적 모델 재호출·budget 환불 없음 | 명시적인 model-call recovery |
| 승인 후 catalog/material/profile 변경 | 이전 승인으로 새 실행 조건 적용 금지 | configuration mismatch, effect 0회 |

Fault trigger는 로그의 우연한 문자열이나 임의 sleep에 맞추지 않고, 준비된 test adapter/store boundary에서 rendezvous 후 process를 종료한다. 외부 side effect는 localhost test sink에 한정한다. Credential·raw provider 응답을 결과 파일로 내보내지 않는다.

Batch의 한 Step이 failed/manual이 되는 probe에서는 descendants 차단과 독립 sibling의 처리 여부를 각각 기록한다. 현재 Core는 실행 가능한 독립 action을 먼저 처리할 수 있다. Batch 전체 원자성이나 첫 실패 시 모든 Step 중단을 암묵적 기대값으로 두지 않는다. 별도 실행 정책이 필요한지 이 결과로 판단한다.

### 2. 동일 모델의 계획 크기 비교

8개 고정 instance × X0/X1/XN × 5회 = 120개의 정상 trial을 기본 pilot으로 둔다. 단일 작은 pilot이므로 일반적인 superiority나 통계적 유의성을 주장하지 않는다. 호출 승인 후 같은 provider/model과 요청 옵션을 고정하고 실행 순서를 균형 있게 섞는다. 실패 후 유리한 trial로 교체하거나 best-of-N만 보고하지 않는다.

모든 조건에 같은 최대 model-call·tool-call·step·context·output bound를 적용한다. X1에 불리하도록 model-call budget을 낮추지 않는다. XS에서 필요한 calls를 확인한 뒤 fixture 작성 단계에 같은 budget 값을 고정한다. 실제 단가·token usage 가용성·총 평가 비용 상한도 API 호출 전에 manifest에 기록한다. 한도 소진은 완료 실패로 남기고 부가 진단을 제공한다.

설계 trial에서 wrapper나 prompt를 수정한 뒤 최종 config hash를 잠그고 validation을 실행한다. 검증 중 정책을 바꾸면 기존 결과는 exploratory로 분리하고 새 validation을 요구한다. Normal trial의 precision·completion을 확인한 뒤 fault trial을 별도로 수행한다.

### 3. 외부 엔진 비교

동일한 model·tool contract를 사용하는 LangGraph 구현부터 검토한다. 각 effect를 별도 task에 두고, durable checkpointer와 sink idempotency/reconciliation을 적용한다. 의도적으로 상태를 저장하지 않는 약한 baseline을 만든 뒤 xgen의 우위를 주장하지 않는다.

Temporal/Restate는 동일 sink를 사용하는 독립 worker/handler 구현으로 비교한다. Version, task boundary, retry 정책, 수동 reconciliation 정책과 운영 구성을 사전에 고정한다. Retry=1 조건과 idempotent retry 조건을 나눠, 안전하게 멈추는 설정도 대조한다. 한 Parent Run에 두 engine의 재시도 loop를 겹치지 않는다.

Codex·Claude SDK는 자체 agent loop·model 제약을 가지므로 별도 제품 비교군이다. 같은 provider/model을 사용할 수 없으면 모델과 하네스의 효과를 분리했다고 주장하지 않는다. 동일 작업·승인 범위·비용 상한으로 사용자 결과를 비교하되 architecture ablation과 별도 표로 보고한다. SDK resume를 external effect exactly-once로 간주하지 않는다.

단계 3의 candidate binary·package version과 실행 환경은 아직 고정하지 않았다. 이를 완료하기 전 외부 엔진 benchmark를 시작하지 않는다. 서버를 설치하거나 외부 SDK를 호출한 결과는 이번 문서 작성에 포함되지 않는다.

## 지표와 판정

| 지표 | 측정 기준 |
|---|---|
| 최종 상태 정확도 | 독립 test/sink/artifact oracle이 요청의 완료 조건을 만족하는가 |
| 잘못된 완료 보고 | 미충족 상태에서 완료·성공을 보고했는가. 허용된 미완료 보고와 구분 |
| duplicate applied effect | 동일 요청의 동일 logical effect가 sink에서 2회 이상 적용됐는가 |
| recovery completion | fault 뒤 original Run에서 완료했는가. 새 Run으로 교체한 성공은 제외 |
| unresolved/manual intervention | 자동 해결 못 한 상태 수, 복구 명령·승인 횟수, operator 소요시간 |
| 모델 사용량 | possible-send reservations, accepted/rejected/unknown calls, input/output tokens |
| 비용 | 확정된 usage와 실제 단가의 비용. Usage missing/추정치는 별도 표시 |
| latency | 정상 end-to-end와 fault recovery를 분리한 median/p95, timeout 포함 |
| 정책 정확성 | 미승인·변경된 resource effect 수와 schema/profile mismatch 우회 수 |

Pilot 채택 gate는 최소 두 validation family에서 XN의 완료 성공 수가 X1보다 적지 않고, unsafe duplicate·미승인 실행·새 false completion이 0이며, 전체 validation median 모델 비용이 15% 이상 감소하는 것이다. Token usage나 단가가 없어 비용을 계산할 수 없으면 비용 gate는 `UNKNOWN`이고 채택하지 않는다. 더 적은 호출 수만으로 비용 감소를 대신하지 않는다. Latency와 manual intervention이 악화하면 원인을 해결한 새 validation 전까지 보류한다.

이 기준은 사전 제안이며 5회 반복 pilot의 통계적 증명이 아니다. Gate 통과는 더 큰 검증과 별도 구현 ADR로 진행할 근거다. 현장·모델·작업 전반의 superiority 주장에는 상위 평가 프로토콜의 반복 수·통계·환경 검증을 적용한다.

Typed output reference나 병렬 실행의 효과는 X1/XN 결과와 섞지 않는다. 구현하려면 각각 새 baseline·adversarial reference cases·입력 snapshot/충돌 검사와 held-out gate를 작성한다.

## 결과물과 다음 실행 단위

첫 구현 단위는 **모델 없는 계획 경계 평가 runner**다. Product runtime을 바꾸지 않고 fixture 생성, scripted planner, sink attempt ledger, fault rendezvous, per-trial report를 묶는다. 그 다음 X0/X1/XN 모델 pilot을 실행한다.

기록은 fixture/config/binary/source hash, engine/version, split, trial ID, scenario, fault boundary, status, oracle 결과, call·token·effect·Receipt 수, 비용·latency·manual intervention을 포함한다. Raw workspace content와 credential은 보고서에 넣지 않는다. 미실행·미구현 조건에는 0점 대신 `NOT_RUN`/`NOT_IMPLEMENTED`를 사용한다.

이 문서는 실행 계획이며 성능 개선·외부 비교가 이미 완료됐다는 기록이 아니다.
