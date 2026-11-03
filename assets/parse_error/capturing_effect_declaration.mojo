# The removed `capturing` effect rejects on a `def` declaration too.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
def main():
    var k = 1

    def run() capturing -> Int:
        return k

    print(run())
