# expect: invalid mode: "z". Can only be one of: {"r", "w", "rw", "a"}
# `open` rejects a mode outside {"r", "w", "rw", "a"} with upstream's text.
def main() raises:
    var f = open(String("mojito_never_created.txt"), "z")
