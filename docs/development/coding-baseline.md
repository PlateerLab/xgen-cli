# 실제 모델 코딩 회귀 기준선

공개 `xgen run/resume`을 고정 과업으로 반복 실행하는 일반 평가다. Runtime·prompt·기본 모델은 바꾸지
않는다. 실행기는 [evaluate-coding-baseline.py](../../scripts/evaluate-coding-baseline.py), 과업 정본은
[cases.json](../../evals/coding-baseline/cases.json)이다. 기존 planning pilot의 비용 proxy·reservation·
process tree 정리와 Receipt 관찰 코드를 재사용한다.

## 과업과 판정

| 종류 | 설계 사례 | 보류 사례 | 추가 판정 |
| --- | --- | --- | --- |
| 저장소 질문 | 설정과 분기에서 실행 경로·batch 크기 찾기 | 여러 파일에서 기본값과 override 결과 찾기 | 요청한 JSON 답과 독립 정답 일치, 파일 변경 없음 |
| 버그 수정 | 빈도 정렬 | 반열린 구간 경계 | 기존 테스트 보존, 독립 경계값 검사 |
| 여러 파일 이름 변경 | dataclass 필드·직렬화 키 | 함수 정의·import·호출부 | 전체 사용처 갱신, 기존 동작 보존 |
| 실패 복구 | flag parser | 행렬 전치 | 같은 검증 명령의 실패 후 성공을 Receipt로 관찰 |
| 승인 후 재개 | 정수 나눗셈 올림 | 공백 제거 후 빈 줄 제외 | 실행 승인 전 process 출력 0개, 같은 Run 재개 |

기본 반복은 사례별 2회, 전체 20 trial이다. `seed`로 순서를 고정한다. 모두 Python 표준 라이브러리의
작은 fixture이며, 실제 사용자 저장소·다른 언어·TUI·외부 API·장기 작업의 성능을 대표하지 않는다.
기존 [사용자 파일럿](rc3-developer-preview-pilot.md)을 대체하지 않는다.

합격은 다음 조건을 모두 충족해야 한다.

- CLI 정상 종료와 durable task completion.
- 변경 허용 파일만 변경하고, 기존 테스트와 입력 파일 보존.
- 별도 oracle 통과와 과업별 추가 판정 통과.
- Unknown·동일 effect 중복 시작 0건.
- endpoint·workspace·executable 없이 완료 Run을 offline replay하고 journal·usage 파일 불변.

`false_completion`은 완료 후보가 있는데 oracle 또는 파일 변경 범위 검사가 실패한 경우다. 최종 응답의
임의 자연어 주장 전체를 채점하는 지표는 아니다. 질문은 과업에 지정한 JSON 답을 검사한다. 관찰 불가를
정상이나 실패 0건으로 바꾸지 않고 `evidence_complete=false`, `false_completion=null`로 남긴다.
정답과 oracle은 모델의 workspace·goal·context에 넣지 않는다. 이는 OS sandbox나 악의적 정답 탐색에
대한 방어를 검증하는 평가가 아니다.

## 실행 준비

Linux와 `pidfd` 지원 Python, 평가할 xgen 실행 파일이 필요하다. Repository source는 변경하지 않고
새 비공개 출력 폴더 아래에 fixture·state·HOME·config를 만든다. 사용자 프로필과 지침을 가져오지 않는다.
배포본을 평가하려면 해당 배포본을 `--binary`로 넘긴다. Local build 결과를 배포본 결과로 표기하지 않는다.

```bash
cp evals/coding-baseline/config.example.json /tmp/xgen-baseline-config.json
chmod 600 /tmp/xgen-baseline-config.json
```

예제의 `null`은 실행 가능한 기본값이 아니다. 로컬 config에서 exact model·tokenizer, 공급자가 보장하는
입력 token 상한, 가격 출처·유효 UTC 구간·단가를 채운다. 오래된 pilot의 quote를 복사하지 않는다.
단가는 1M tokens당 정수 micro USD, 지출 상한과 결과 금액은 정수 nano USD다.
예제 `spend_cap_nano_usd=2000000000`은 $2다. 모델별 response format·thinking과 출력 상한도 확인한다.

`upper_bound`는 공급자 단가 상한으로 계산하고 실제 비용은 UNKNOWN으로 남긴다. `fixed_tier`는 해당
구간에 고정된 단가를 확인했을 때만 쓴다. 비용 분류는 일반 입력·캐시 입력·출력 세 가지를 지원한다.
별도 cache-write 요금 등 이 분류로 표현할 수 없는 과금은 이 실행기의 실제 비용으로 주장하지 않는다.

API key는 실행하는 터미널의 `XGEN_OPENAI_API_KEY` 환경변수로만 공급한다. 입력 예시는 숨김 입력을
사용하며 명령행 인자나 config에 값을 넣지 않는다.

```bash
read -rs -p 'API key: ' XGEN_OPENAI_API_KEY
export XGEN_OPENAI_API_KEY
read -r -p 'OpenAI-compatible HTTPS API root (/v1): ' XGEN_EVAL_BASE_URL
export XGEN_EVAL_BASE_URL
python3 scripts/evaluate-coding-baseline.py \
  --binary /absolute/path/to/xgen \
  --config /tmp/xgen-baseline-config.json \
  --split all \
  --output /tmp/xgen-baseline-result
unset XGEN_OPENAI_API_KEY
```

출력 폴더는 매번 새 경로여야 한다. API key는 host proxy에만 있고 xgen·등록 실행 파일·oracle의 환경,
manifest·Receipt·모델 context에는 전달하지 않는다. 연결 주소도 결과 manifest에 저장하지 않는다.
기존 secure-store 프로필을 자동으로 읽지 않으므로 `xgen model setup`만으로 이 실행기의 인증이 준비되지는
않는다. 인증값은 채팅이나 저장소에 넣지 않는다.

## 예산과 결과

요청 전 기존 Ledger가 최악의 cache-miss 입력 상한과 출력 상한 비용을 예약하고 fsync한 뒤 전송한다.
확인된 비용과 미정산 예약을 합쳐 cap을 판정한다. 응답 오류·usage/cache 누락·timeout의 예약은 환급하지
않으며 다음 trial로 넘어가지 않는다. Quote 만료·예산 부족·model/fingerprint drift도 기록하고 중단한다.
자동 trial 재실행은 없다. Runtime 자체의 정해진 proposal 재시도는 실제 모델 호출과 비용에 포함된다.
요청 직렬화 크기로 token 수를 추정하지 않는다. 이 예산 경계는 공급자가 선언한 상한과 요금을 지킨다는
전제이며, 이미 발생한 청구를 되돌리거나 공급자 invoice를 검증하지 않는다.

결과에는 binary·Python·runner·config·case hash와 전체 예정 순서, 모든 실행의 판정·단계 수·반복·시간·
usage·비용을 남긴다. `repeated_finished_actions`는 같은 capability·version·인자의 재실행이고,
`repeated_identical_results`는 결과 digest까지 같은 재실행이다. 정상적인 수정 후 재검증도 첫 지표에는
포함될 수 있으므로 단독 실패 기준으로 쓰지 않는다. Input/output/cache 수치는 확인된 부분합이고
`complete_token_usage=false`이면 전체 사용량으로 보고하지 않는다.

`summary.json`은 split·과업별 시도/성공·잘못된 완료·관찰 불가·중앙 시간·반복·비용을 집계한다.
`trials_executed < trials_planned`이면 미실행 과업이 있고, `complete_schedule=false`다.
모델 응답·source·도구 출력·state DB를 공유 결과에 복사하지 않는다. 출력 디렉터리 자체는 실행 증거와
fixture source를 포함하는 비공개 자료다. 공유할 때는 `summary.json`의 비민감 집계만 사용한다.

## 변경 비교 규칙

1. 기준선 binary·설정·과업·반복·예산을 실행 전에 고정한다. 모델별 결과를 섞지 않는다.
2. 변경 개발 중에는 `--split design`만 실행한다. 서로 다른 최소 두 사례에서 문제를 확인한다.
3. 후보를 고정한 뒤 `--split held_out`으로 기준선과 후보를 같은 조건에서 비교한다. 보류 결과를 보고
   다시 수정했다면 그 과업을 다음 채택 판정의 보류 사례로 재사용하지 않는다.
4. 실패·거절·timeout·미실행을 제외하지 않는다. 완료율·잘못된 완료·관찰 누락·전체 비용을 함께 보고,
   토큰이나 성공 사례 하나만으로 개선을 주장하지 않는다. 작은 표본으로 통계적 우월성을 주장하지 않는다.

## 구현 검증과 실제 모델 결과

2026-11-20: 고정 10개 fixture의 원본 실패·알려진 수정 성공과, 실제 public CLI에 연결한 loopback mock의
정상 10 trial·잘못된 완료 10 trial을 검증했다. Mock usage·가격은 합성 값이며 모델 품질 결과가 아니다.
실제 활성 모델의 catalog 검사는 `authentication_rejected`로 끝났다. 실제 모델 기준선은 인증과 유효한
quote를 준비한 뒤 실행해야 하며, 현재 완료율·토큰·금액 측정 결과는 없다.

로컬 전체 Python 검사 168개 중 112개 통과·56개 조건부 skipped, 새 baseline 검사 6개는 모두 통과했다.
공개 문서 계약 검사도 통과했다. 새 fixture는 원본에서 실패하고 알려진 수정에서 통과하는지 검사했으며,
공통 DB 관찰 함수의 connection을 명시적으로 닫아 반복 실행 시 ResourceWarning도 제거했다.

## 공식 CLI 경유 별도 평가

[agy 연결 평가](agy-coding-baseline.md)는 같은 과업·oracle·run/resume을 재사용한다. 공식 CLI의
로그인과 bounded proposal transport를 쓰며, 위 API 경로의 가격·지출 예약 계약과는 구분한다.
