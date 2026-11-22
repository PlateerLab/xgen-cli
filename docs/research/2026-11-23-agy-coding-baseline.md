# agy 제안 + xgen 실행: 고정 코딩 20회 기준선

2026-11-23. **고정한 20회 중 19회 통과했다.** 별도 모델 API key 없이 공식 agy CLI의 Gemini 제안을
받아 xgen의 파일·process 도구와 승인·재개를 실행했다. 질문·수정·복구·승인 흐름이 이 작은 평가에서는
동작했다. 정식 provider 채택이나 Claude Code/Codex 대비 우위를 입증하는 결과는 아니다.

일반 평가 연결기의 구현·사용법은 [개발 문서](../development/agy-coding-baseline.md), 비민감 집계와
고정 입력 hash는 [결과 JSON](2026-11-23-agy-coding-baseline-results.json)에 있다.

## 조건과 결과

기존 10개 합성 Python 과업을 각각 2회, seed 20261120으로 실행했다. 모델은
`gemini-3.8-flash-low`, xgen은 rc.8 runtime의 로컬 debug binary다. 공개 release artifact 자체의
품질 평가가 아니다. 실행 전에 binary·agent·설정·runner/helper를 고정했으며 종료 시 모두 불변이었다.
도중 설정·과업·oracle을 변경하지 않았고, 실패한 trial을 다시 실행하지 않았다.

| 종류 | 설계 | 보류 | 합계 |
| --- | --- | --- | --- |
| 저장소 질문 | 2/2 | 2/2 | 4/4 |
| 버그 수정 | 2/2 | 2/2 | 4/4 |
| 여러 파일 이름 변경 | 2/2 | 2/2 | 4/4 |
| 실패 복구 | 2/2 | 2/2 | 4/4 |
| 승인 후 재개 | 1/2 | 2/2 | 3/4 |
| 전체 | 9/10 | 10/10 | 19/20 |

합격은 정상 종료·durable completion·독립 oracle·변경 범위 보존·Unknown/중복 effect 0·offline replay를
모두 충족한 경우다. 승인 trial은 실행 승인 전 process 출력 0과 같은 Run 재개를 확인한다. 실패 복구는
같은 검증 명령이 수정 전 실패하고 수정 후 성공해야 한다. 단순한 모델의 완료 선언으로 합격시키지 않았다.

- CLI 논리적 시도 98회, xgen Receipt 78개. Proposal 검증 거절 0개.
- 잘못된 완료 0회, 중복 effect 시작 0회, effect Unknown 0개.
- Model-call Unknown 1개: 아래 native 도구 호출을 중단한 응답이다. 이를 정상/0으로 바꾸지 않았다.
- 같은 인자 행동 반복 4개는 실패 복구에서 테스트를 수정 후 다시 실행한 것이다. 동일 결과 반복은 0개.
- Trial 시간 합 약 805초(13.4분), 중앙값 34.4초. 시간 합은 사전 인증·discovery 검사 시간을 제외한다.
- 종료 code 1은 20회를 모두 실행했지만 1회가 실패했다는 뜻이다. 스케줄 미완료나 측정기 crash가 아니다.

## 실패 한 건

`resume-rounding` 두 번째 반복의 CLI 시도 37에서 agy 자체 `manage_task(Action=list)`가 실행됐다.
하나의 native step에 ACTIVE/DONE 두 transition을 관찰했다. 연결기는 관찰 직후 해당 process tree를
중단하고 `native_tool_observed`로 실패 처리했다. DONE까지 같은 수신 chunk에 있었으므로 이 차단이
도구의 실행 자체를 사전에 막았다고 말할 수 없다. xgen은 응답을 transport unavailable로 기록했다.
파일 변경 범위는 보존됐고 승인 전 process 출력도 0개였다.

다른 반복에서는 같은 과업이 승인·재개·완료를 통과했다. 설정만으로 agy 내부 도구를 전부 제거할 수
있다는 주장은 채택하지 않는다. 이전 연결 smoke와 이번 평가의 서로 다른 경로에서 도구 잔존을
관찰했으므로, 정식 연결 전에 해결하거나 명시적으로 수용할 일반 transport 과제다.
`summary.native_tool_events=1`은 native 활동으로 거절한 CLI 시도 수이며 stream transition 수가 아니다.

## 사용량과 비용의 한계

최종 usage가 있는 97회에서 CLI가 보고한 부분합은 input_tokens 875,435, output_tokens 20,274,
cache_read_tokens 20,392, thinking_tokens 9,957이다. 중단한 1회는 최종 usage가 없다.
Thinking은 output의 부분집합으로 취급하며 별도로 더하지 않는다. 공급자 원시 응답이나 청구서를 확인한
토큰 총량이 아니다.

두 응답은 cache_read_tokens가 input_tokens보다 컸다(7,003 대 2,087, 5,234 대 2,789). Input이
cache를 제외한 값인지 등 의미를 확인하지 못했으므로 OpenAI 방식으로 더하거나 환산하지 않았다.
연결기의 정규화 cache 값은 두 회 모두 UNKNOWN이며, 확인된 cache 부분합 8,155만 summary에 남는다.
원본 CLI cache 부분합 20,392도 별도로 보존했다. 따라서 전체 usage가 완전한 결과는 아니며, 3개 시도의
정규화 usage가 불완전하다. **USD 비용과 provider 내부 retry/request 횟수는 UNKNOWN이다.**

## 채택 범위와 다음 판단

- `ADOPT_WITH_GATES`: 별도 API key 없이 기존 고정 평가를 실행하는 개발용 연결기. 시간·bytes·논리적
  호출 한도, native 관찰 후 실패 처리, 엄격한 JSON, 실패·UNKNOWN 보존을 포함한다.
- `DO_NOT_ADOPT`: agy를 raw model API로 간주하기, agy 도구·memory 전체 자동 이전, native 실행의 사전
  완전 차단·정확한 USD cap을 보장한다는 주장. 정식 provider/profile/resume drift 계약도 아직 없다.
- 이 결과로 runtime·prompt·배포본을 바꾸지 않았다. 모델·하네스 개선의 전후 비교도 하지 않았다.
  다음은 native 도구 억제 가능성과 usage 의미를 agy 경로에서 확인하고, xgen 쪽 정식 transport의
  변경·실패·재개 계약을 따로 판단하는 것이다.

작은 Python fixture 10개·각2회다. 실제 대형 저장소·장기 작업·다른 언어·UI·HTTP 과업이나 일반 사용자
정확도를 대표하지 않는다. Claude Code/Codex를 같은 조건으로 재지 않았으므로 상대 성능은 미확인이다.
향후 하네스 변경은 최소 두 설계 사례의 근거와 별도 미검토 보류 사례로 검증한다. 이번에 분석한 사례를
새로운 독립 보류 자료라고 주장하지 않는다. 인증값·계정·대화 ID·사용자 설정 원문은 공개 기록에서 제외했다.
