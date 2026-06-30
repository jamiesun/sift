import subprocess
from setuptools import setup

subprocess.check_call(["sh", "-c", "echo synthetic setup hook"])

setup(name="fixture-python-setup-command", version="0.0.0")

