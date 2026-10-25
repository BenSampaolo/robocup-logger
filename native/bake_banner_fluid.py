import subprocess
import tempfile
from pathlib import Path

source = Path(__file__).resolve().with_suffix(".rs")
with tempfile.TemporaryDirectory(prefix="robocup-fluid-bake-") as directory:
    binary = str(Path(directory) / "bake-fluid")
    subprocess.run(["rustc", "--edition=2024", "-O", str(source), "-o", binary], check=True)
    subprocess.run([binary, str(source.parents[1] / "src/banner_fluid.bin")], check=True)
