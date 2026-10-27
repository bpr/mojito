# expect: cannot transfer out of immutable reference
# An `imm` capture is an immutable reference to the enclosing binding.
def main():
    var s = String("a")
    def inner() {imm s} -> String:
        return s^
    print(inner())
