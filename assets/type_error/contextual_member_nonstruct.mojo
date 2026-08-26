# expect: does not supply struct members
# A leading-dot member against a non-struct expected type rejects (upstream
# reports "'Int' value has no attribute 'red'"; Mojito rejects before member
# lookup since builtins carry no static members).
def main():
    var n: Int = .red()
