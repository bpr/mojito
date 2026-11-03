# expect: '@__copy_capture' is no longer supported; list the captures in a capture list like '{var x}' instead
# The removed `@__copy_capture` decorator rejects with its own text: a
# capture list names the copied capture.
def main():
    var k = 3

    @__copy_capture(k)
    def get() -> Int:
        return k

    print(get())
