# Run별 capability 범위

`run --capability ID`로 계획과 실행에 등록할 capability ID를 제한해. 여러 번 지정할 수 있어.
옵션을 생략하면 기존 전체 catalog를 유지해. 특정 업무나 모델에 종속되지 않는 일반 기능이야.

```sh
xgen run --workspace . --allow-dir . --allow-read --allow-remote-model-egress \
  --capability xgeny.fs/read-text 'README.md를 읽고 요약해'
```

선택은 원래 등록 가능한 정의와 instance의 부분집합이야. 모르는 ID·중복 ID는 Run을 만들거나
모델에 요청하기 전에 거절해. capability를 선택해도 파일·HTTP·실행 승인 조건은 그대로 필요해.
모델이 목록 밖의 작업을 제안하면 admission에서 거절해.

정렬된 목록을 manifest, authority, 실행 profile digest에 결속해. `resume`은 저장된 범위를
복원하고 새 범위 옵션을 받지 않아. 기본값은 manifest에 새 필드를 저장하지 않아 이전 Run과
전체 catalog 경로를 유지해. 도구 반환값·Receipt·원문은 수정하거나 요약하지 않아.

HTTP read만 선택한 호스트는 `tools describe`로 확인한 정확한 요청·contractDigest를 goal에
전달할 수 있어. describe가 범위 밖이고 계약도 없다면 에이전트는 선행조건 부족을 보고해야 해.
범위 선택은 부족한 계약을 추측하거나 HTTP 권한을 부여하는 기능이 아니야.

검증: CLI library109개·workspace integration22개 통과, clippy 경고0. 새 검사에서 읽기/상태
조회 각각의 승인 전 정지·재개·완료 재열기, write 승인이 있어도 범위 밖 쓰기 거절,
잘못된 ID·중복·manifest 변경 차단을 확인했어. 실제 모델의 판단 품질 향상은 검증하지 않았어.
