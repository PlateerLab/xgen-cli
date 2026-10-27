# ADR-0072: planning context에서 toolOutputs를 steps 앞에 둔다

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용 planner wire 순서. 내용·canonical digest 변경 없음. ADR-0036 §1의 순서를 대체한다.

## 문제

긴 작업에서 DeepSeek prefix cache 적중이 35%에 그쳤다. H6·H7·T3 8회 실행(360 call)의 요청을 나누면 call당
평균 43,000 token(149 KB) 중 `toolOutputs`가 75%, `capabilities` 14%, `steps` 6%였다.

ADR-0036은 `steps`와 `toolOutputs`가 둘 다 뒤에만 자란다고 보고 `steps`를 앞에 두었다. 실제로는 Step 상태가
제자리에서 바뀐다(planned → completed/failed). 연속한 두 요청이 처음 달라지는 위치는 132쌍 중 120쌍이 `steps`
안이었다. 그래서 그 뒤의 `toolOutputs` 전체가 매번 cache 밖이었다. 반대로 `toolOutputs`는 360 call 내내 뒤에만
붙었다(중간 삭제·재배열 0회).

## 결정

Context wire 순서를 `profileVersion`, `capabilities`, `omittedCapabilities`, `catalogDigest`, `goal`,
`planningConstraints`, `toolOutputs`, `steps`, `omittedSteps`, 그 뒤 가변 식별자로 바꾼다.

- 내용과 RFC 8785 `context_digest`는 그대로다. journal·receipt·manifest digest는 바뀌지 않는다.
- 순서는 계약이므로 envelope profile을 `xgeny.planner-request/v3`으로 올린다. request profile digest가 바뀌므로
  이 변경 전에 시작해 끝나지 않은 Run은 resume 때 `configuration_mismatch`로 닫힌다(ADR-0036 §2와 같다).
- ADR-0069의 `supersededBy` 표시는 옛 출력에 붙어 prefix를 한 번 줄이지만, 별도 목록으로 빼는 안은 사전 계산에서
  91.1% → 91.5%로 차이가 없어 하지 않았다.

## 검증

사전 계산: 기록된 360개 요청을 순서만 바꿔 연속 call의 공통 prefix를 재면 35.7% → 91.1%다. 현재 순서의 계산값
35.7%가 실제 적중 35%와 맞았다.

실행(deepseek-flash, `--trust`). 설계 사례 H6·T3, 검증 사례 H7과 이름 변경이 아닌 H4·T1:

| 작업 | 기준 cache 적중 | 변경 후 | 기준 cache 밖 입력 | 변경 후 |
|---|---|---|---|---|
| H6 (3회) | 49~62% | 85~89% | 0.57~0.98M | 0.08~0.20M |
| H7 (3회) | 27~34% | 86~90% | 0.76~0.88M | 0.13~0.16M |
| T3 (2회) | 21~34% | 90~91% | 2.55~2.64M | 0.20~0.28M |
| H4·T1 (짧은 작업) | 72~88% | 69~83% | 변화 없음 | |

결과 품질:
- H6: 기준 3/3이 `repeated_finished_action`으로 멈춤(남은 사용처 17/20/12) → 변경 후 0/3 멈춤, 남은 5/5/13.
- H7: 기준 3/3 완료 → 2/3 완료(1회는 twitter.py 2곳 남김). 이전 빌드들도 0~9곳으로 흔들렸다.
- T3: 남은 5/0 → 3/7.
- H4 2/2 정답, T1 2/2 테스트 통과.
- H6 1회가 `output_truncated`로 끝났다. 같은 edit를 반복해 출력 상한까지 쓴 응답이다. 지금까지 기록된 실행에서 처음
  본 1건이라 순서 변경과의 관계는 판단하지 않는다.

모델이 보는 내용이 같으므로 완료율 차이는 표본 변동 안에 있다고 본다. 비용 효과는 긴 작업에서 cache 밖 입력이
약 1/5~1/10이다.
