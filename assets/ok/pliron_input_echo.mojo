# Native `input()`: the prompt writes to stdout without a newline before
# mjrt_read_line consumes one stdin line (trailing newline stripped), and
# EOF yields the empty string instead of blocking — a second read on
# exhausted stdin prints an empty tail. The parity harness pipes the same
# bytes to the native executable's stdin and to the VM's test-only input
# override, which captures prompts in the compared output.
def main():
    var first = input("first: ")
    var second = input("second: ")
    print("got", first)
    print("tail", second, "end")
