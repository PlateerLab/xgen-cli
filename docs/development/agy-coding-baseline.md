# agy를 통한 고정 코딩 평가

일반 평가 연결기다. 공식 agy CLI에서 proposal을 받고, 실제 파일·process 도구와 승인·기록·재개는
xgen이 맡는다. 정식 provider나 사용자용 로그인 기능을 추가한 것은 아니다. agy 내부 runtime은 남아
있으며, 모든 도구·memory를 이전하거나 raw model API와 같은 통제를 보장하지 않는다.

[기존 기준선](coding-baseline.md)의 고정 과업·순서 생성·독립 oracle·공개 run/resume·offline replay를
재사용한다. API 평가의 가격표나 지출 예약을 이 경로에 적용하지 않는다. USD와 공급자 내부 요청 횟수는
UNKNOWN이다. CLI 호출 수는 durable reservation을 만든 논리적 시도 수이며 launch 실패도 포함한다.

## 실행

Linux pidfd 지원 Python, 평가할 xgen binary, 공식 agy CLI와 로컬에서 완료한 로그인이 필요하다.
인증값을 채팅·명령행·저장소에 넣지 않는다. 모델 catalog와 custom agent discovery를 먼저 검사한다.

```bash
python3 scripts/evaluate-agy-coding-baseline.py \
  --binary /absolute/path/to/xgen \
  --agy /absolute/path/to/agy \
  --output /tmp/xgen-agy-new-batch
```

[고정 설정](../../evals/coding-baseline/agy-config.json)은 10개 과업을 각각 2회 실행한다. 과업/정답은
[기존 정본](../../evals/coding-baseline/cases.json)을 사용한다. 사용자 API key 환경변수를 자식에게
물려주지 않고, agy가 기존 HOME의 로그인 상태를 사용하는 데 필요한 환경만 남긴다. xgen fixture의
HOME·state·workspace는 분리한다. 전역 hooks가 있으면 실행을 거절한다. 기존 설정을 삭제하지 않는다.

평가용 agent는 기본 component/MCP/customization 상속을 끄고 tools/rules/hooks를 빈 목록으로
선언한다. 별도 설정에 command/read/write/URL/MCP deny 규칙을 넣는다. 선택 agent/model을 매 호출
init에서 검사한다. native tool/subagent event를 관찰하면 해당 프로세스 tree를 중단하고 실패로 남긴다.
**이것은 관찰 후 차단이며, 내부 동작이 시작되기 전에 모두 막았다는 보장은 아니다.**
`init.tools` 목록만으로 유효 도구가 0개라고 판정하지 않는다.

## 한도와 판정

- batch 30분·최대 CLI 시도 200회, trial 4분·최대 16회, 요청 60초다. trial 자동 재실행은 없다.
- prompt 100,000 bytes, stdout 131,072 bytes, stderr 32,768 bytes로 제한한다. 초과·timeout 시
  자식 process tree를 중단한다. xgen의 host output 설정 4,096 tokens는 native agy 출력 token 상한을
  강제하지 못한다. 실제로 강제하는 한도는 시간·bytes·논리적 CLI 시도 수다.
- binary 두 개를 복사하고 runner/helper·agent·설정의 hash를 고정한다. 변경을 관찰하면 중단한다.
  config/cases는 메모리와 manifest에 snapshot한다. agy 서버 측 모델 변경까지 고정하지 못한다.
- stdout의 NDJSON과 최종 단일 JSON에서 중복 key·비유한값을 거절한다. 반환 JSON은 재작성하지 않고
  xgen의 원래 proposal validator로 넘긴다. transport 성공은 과업 합격을 뜻하지 않는다.
- 오류가 난 시도도 ledger와 trial 결과에 남긴다. 전체 스케줄 미완료와 usage 누락을 숨기지 않는다.
  native event·잘못된 응답 등 transport 거절이 한 번이라도 있는 trial은 보수적으로 실패 처리한다.
- CLI 보고 input/output/cache를 검증하고 확인된 부분합만 집계한다. 누락은 0으로 채우지 않는다.
  reasoning은 원본 ledger에 별도 보존하며 output의 부분집합으로 취급한다. USD 계산에는 사용하지 않는다.

출력 폴더는 비공개다. 모델 응답·fixture·state DB가 있으므로 폴더 전체를 공유하지 않는다. 공개 기록에는
집계·제한·비민감 hash만 옮긴다. 성공만 골라 재측정하거나 실패 후 같은 batch를 덮어쓰지 않는다.

## 구현 검증

2026-11-23: Python 전체 175개 검사에서 119개 통과·56개 skip. 실제 public xgen과 합성 agy process를
연결해 execute 승인 전 정지·같은 Run 재개·offline replay를 검사했다. native event 조기 중단, timeout의
자식 process 정리, 출력 한도, durable 호출 한도, pin drift, usage 누락도 별도로 검사했다.
이는 실제 모델 품질 검증과 구분한다.
