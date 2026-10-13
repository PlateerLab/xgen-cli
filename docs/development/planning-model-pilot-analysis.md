# 계획 크기 pilot 결과 분석

범용 평가 도구 `scripts/analyze-planning-pilot.py`는 종료된 실험의 JSONL과 SQLite를 읽어. 모델을 호출하거나 후보 파일·Run을 수정하지 않아. 실행 중인 실험의 runner·provider·fixture를 수정하는 용도로 쓰지 않아.

```bash
python3 scripts/analyze-planning-pilot.py \
  --results /path/to/private/completed-pilot \
  --preregistration evals/planning-model/preregistration-2026-10-05.json \
  --output /path/to/analysis.json
```

`summary.json`이 만들어진 뒤 실행해. 공개 결과에는 집계·ledger·manifest·분석·원본 journal의 hash가 있고, 전체 SQLite와 바이너리는 별도 로컬 자료야. 위 CLI의 전체 audit를 다시 실행하려면 그 로컬 자료가 필요해. 공개 hash만으로 journal 내용을 독립 검증할 수 있다고 주장하지 않아.

## 집계 계약

- Split·family·case·조건별 실행 수, 실제 사례 수, 작업 oracle, 응답 계약, 모델 호출, 시간, 비용을 분리해. `accepted`는 작업·응답·수정 범위·보호 파일·중복 실행 검사를 모두 통과한 수야.
- 실패 실행을 제외하지 않아. 비용 미확인은 0으로 대체하지 않고 median도 UNKNOWN으로 남겨.
- 동일 case/repeat의 X1·XN을 paired block으로 표시해. 5회 반복을 독립 task 5개로 해석하지 않아.
- 등록한 전체 순서, source와 binary, config, fixture digest가 일치하는지 확인해. Source 유지 여부는 runner의 종료 검사도 필요해.
- 사용량의 cache hit·miss·output에 등록 가격을 다시 적용해 ledger와 trial·전체 합계를 대조해. Provider 청구서와의 대조는 아니야.
- 등록 gate를 적용하고 결과를 서술적 pilot으로 해석해. 통과해도 production 기본값을 자동 변경하지 않아. 비용 기준은 validation 전체 trial median의 X1 대비 XN 15% 감소야. 조건 또는 family가 전부 실패해서 저렴해진 경우는 채택 근거로 쓰지 않아.

`false_claim`은 이 평가의 strict 응답 계약 위반 지표야. JSON 대신 설명문을 반환한 경우도 포함해서, 모두 사실을 꾸며낸 것으로 읽으면 안 돼. 분석은 형식 위반, 명령 순서, 명령 내용, 변경 파일 불일치를 구분하고 원래 실패 점수는 유지해.

## Journal audit 범위

각 실행 시작보다 앞선 plan·intent·grant가 있는지, 승인 run/step/authority/head/action/capability/instance가 일치하는지, grant 소비 횟수와 material·retained recipe가 맞는지 확인해. 성공 output과 verification Receipt의 invocation·input/output·policy·executor도 대조해. 완료되지 않은 effect나 sidecar 누락은 실패로 남겨.

이것은 관계와 digest 값의 일치 검사야. 모든 canonical digest·signature를 독립 재계산하거나 OS sandbox 경계를 증명하는 보안 검증기는 아니야. 따라서 audit 통과만으로 프로세스가 host의 다른 파일을 읽지 못한다고 결론내리지 않아. Engine의 기존 계약 검사와 외부 작업 oracle를 대체하지도 않아.

## 도구 검사

```bash
python3 -m unittest discover -s scripts/tests -p test_planning_pilot_analysis.py
```

단위 검사는 실패 비용 포함, UNKNOWN 유지, design/validation 분리, ledger 재계산, paired 실패 보존, 응답 형식과 명령 순서의 분리를 확인해. 실제 development SQLite를 제공하면 추가 관계 검사를 실행할 수 있어.

```bash
XGEN_PILOT_AUDIT_FIXTURES=/path/to/private/development-results \
  python3 -m unittest discover -s scripts/tests -p test_planning_pilot_analysis.py
```

추가 검사는 여러 development 사례의 정상 journal을 확인하고, 서로 다른 두 사례에서 승인 action·material·Receipt invocation을 훼손하거나 선행 intent를 제거했을 때 실패하는지 확인해. 원본은 읽기만 하고 mutation은 임시 복사본에 적용해. Private journal이 없으면 이 추가 검사 두 개는 skip이야.
