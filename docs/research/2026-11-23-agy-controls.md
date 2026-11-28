# agy 자체 도구 차단과 usage 의미 후속 확인

2026-11-23. 일반 연결기 과제다. **usage 변환은 수정했지만, 현재 격리 설정에서 native 도구 실행 전
차단은 검증하지 못했다.** 정식 provider나 배포본에는 넣지 않았다. 이전 [20회 평가](2026-11-23-agy-coding-baseline.md)의
19/20 점수·실패·UNKNOWN 기록을 보존하고, 사용량만 원본 응답에서 오프라인 재집계했다.

## usage 변환 수정

공식 [headless 문서](https://antigravity.google/docs/cli/headless/)의 multi-turn 예제에는 input 278,
cache 30,214, output 4, total 282인 응답이 있다. 문서의 수치와 실제 두 응답(input 2,087/cache 7,003,
input 2,789/cache 5,234)을 근거로 **agy의 input/total은 cache를 제외한다**고 해석했다. Thinking은
output의 부분집합이다. 이를 OpenAI의 cache 포함 prompt와 같은 값으로 취급한 이전 변환이 잘못됐다.

이 연결기의 정규화 계약 `agy-uncached-input/v1`은 다음과 같다.

- `uncached_input` = CLI `input_tokens`.
- Host `input`/`prompt_tokens` = CLI `input_tokens` + `cache_read_tokens`.
- `output` = CLI `output_tokens`. Thinking을 다시 더하지 않는다.
- CLI `total_tokens`는 CLI input + output과 검증한다. Host total은 정규화한 input + output이다.
- 누락·음수·bool·잘못된 total을 0으로 채우지 않는다. Cache가 없으면 uncached만 보존하고 inclusive
  input은 UNKNOWN으로 둔다. USD와 provider 내부 요청 횟수는 여전히 UNKNOWN이다.

기존 98개 논리적 CLI 시도 중 97개의 최종 usage를 같은 함수로 다시 읽었다. 모델 I/O는 없고 원본
manifest·ledger·trials·summary hash가 그대로임을 확인했다. 품질 평가를 다시 실행한 것은 아니다.

| 지표 | 수정 후 부분합 |
| --- | ---: |
| 입력, cache 포함 | 895,827 |
| 입력, cache 제외 | 875,435 |
| Cache 읽기 | 20,392 |
| 출력 | 20,274 |
| Thinking, 출력에 포함 | 9,957 |
| 확인된 최종 usage | 97/98회 |
| Usage 미확인 | 1회 |

이전의 cache 이상 2회는 정규화 가정의 오류로 설명된다. Native 호출 중단 1회는 최종 usage가 없어서
계속 UNKNOWN이다. 실제 provider/invoice 총량이나 무료 사용을 입증한 것이 아니다.
[재집계 결과·원본/정규화 코드 hash](2026-11-23-agy-usage-reanalysis.json)를 별도로 남겼다. 과거 JSON과
xgen의 원래 journal/usage DB를 덮어쓰지 않았다.

## 실행 전 차단 진단

공식 [hook 계약](https://antigravity.google/docs/hooks)은 workspace `.agents/hooks.json`의
`PreToolUse`, 전체 도구 matcher, `decision=deny`를 지원한다. 임시 합성 workspace에서 이 경로를
시험했다. `/hooks`는 지정 hook을 enabled로 표시했고, 동일 handler를 직접 호출하면 정상적인 deny
JSON과 audit 행을 남겼다. 사용자 전역 설정은 수정하지 않았다.

모델 진단 8회에서 기본 구성과 명시적인 hook 이름 선택을 각각 시험했다. 현재 연결기처럼
`inheritCustomizations=false`인 agent에서는 hook audit가 생기지 않았고, `manage_task(Action=list)`는
hook 유무와 관계없이 실제 완료했다. 명시적 hook 이름을 넣은 읽기 진단 한 회에서도 실제 파일 내용이
반환됐다. 따라서 이 구성에서 hook 등록 목록을 사전 차단의 증거로 사용할 수 없다.

읽기 진단의 positive control은 요청한 도구를 호출하지 않아 정상 대조군으로 쓸 수 없었다. 이를 차단
성공이나 품질 비교에 포함하지 않았다. 진단의 agent 본문은 도구 호출을 유도하는 별도 문구이며,
production proposal prompt나 이전 20회 평가를 바꾼 것이 아니다. 이 임시 관찰기는 native event를
기록하며 계속 진행했으나, 실제 xgen 연결기의 감지 후 중단 정책은 그대로다.

공식 [plugin 작성 문서](https://antigravity.google/docs/build-plugins/)는 `inheritCustomizations`가
hooks 상속도 제어한다고 설명한다. 현재처럼 상속을 끈 상태에서 workspace hook이 agent에 적용되지
않는 결과와 일치한다. 명시적 hook 이름만으로 이를 해결하지 못했다. 모든 agy 설정에서 hook이
불가능하다거나 hook 기능 자체가 고장났다고 결론 내리지는 않는다. 사용자 전역 customization 상속을
켜서 private rules/skills/plugins를 가져오는 방식은 이번 격리 정책의 해법으로 채택하지 않았다.

[진단 집계·trace hash](2026-11-23-agy-controls-results.json). 유효 도구를 완전히 제거하는 새 공식
스위치를 확인하지 못했다. 기존 custom agent의 빈 tools와 default component 제외만으로는 부족하다는
이전 실측도 유지한다.

## 채택 범위

- `ADOPT`: 개발용 연결기의 usage 정규화와 오프라인 재집계. 공식 예제와 서로 다른 두 실측 응답,
  실제 public run/resume에 연결한 합성 cache 응답으로 검증했다.
- `DO_NOT_ADOPT`: 이번 deny-hook 구성을 native 실행 전 차단으로 취급하기. 실제 적용 증거가 없다.
- `ADOPT_WITH_GATES` 유지: 개발용 agy 평가 경로. Native 관찰 시 프로세스를 중단하고 해당 시도를 실패로
  남긴다. Raw model API·도구 완전 제거·정확한 과금 cap·정식 provider를 보장하지 않는다.

Python 전체 178개 중122개 통과·56개 skip. 원래 API 기준선 집계는 바꾸지 않았고, 누락/잘못된 usage,
cache가 uncached보다 큰 응답, thinking 중복 합산 방지, 실제 host 승인/재개·offline replay,
원본 불변의 usage 재집계를 확인했다. 정식 provider·profile/resume drift 계약은 별도 설계 과제로 남긴다.
