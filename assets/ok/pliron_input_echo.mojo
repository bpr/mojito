# Native `input()`: the prompt writes to stdout without a newline before
# mjrt_read_line consumes one stdin line (trailing newline stripped), and
# exhausted stdin does not block — a second read prints an empty tail. The VM
# and upstream raise `EOF` there, so the reads go through a helper that catches
# it; natively `mjrt_read_line` still returns the empty string until the next
# runtime ABI bump, and the helper's fallback prints the same text. The parity harness
# pipes the same bytes to the native executable's stdin and to the VM's
# test-only input override, which captures prompts in the compared output.
def read_line(prompt: String) -> String:
    try:
        return input(prompt)
    except e:
        return String("")

def main():
    var first = read_line(String("first: "))
    var second = read_line(String("second: "))
    print("got", first)
    print("tail", second, "end")
