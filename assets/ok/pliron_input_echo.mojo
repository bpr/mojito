# Native `input()`: the prompt writes to stdout without a newline before
# mjrt_read_line consumes one stdin line (trailing newline stripped), and
# exhausted stdin yields the empty string instead of blocking — a second read
# prints an empty tail. Upstream raises `EOF` there rather than returning the
# empty string, so the reads go through a helper that catches it; the
# `input-eof` conformance case carries the divergence. The parity harness
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
