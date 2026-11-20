# Prepare step: runs before the agent, without an LLM. The argument is the repository path.
# The example only reports; a real prepare step changes the tree deterministically.
import sys

print(f"prepared {sys.argv[1]}")
