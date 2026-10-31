#!/bin/sh
# Verifier: the Django tests pass on Python 3.10 with the Django 5.2 line the project pins.
set -eu
cd "$1"
uv run --no-project --python 3.10 --with 'django>=5.2,<5.3' python manage.py test
echo "ok: Django tests pass"
