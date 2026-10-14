PY := python3

probe:
	$(PY) tools/probe_runner.py --json out.json

ghost:
	python3 tools/not_there.py

lint:
	./scripts/lint.sh --fix
