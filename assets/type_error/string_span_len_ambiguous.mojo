# expect: StringSpan does not support `__len__`
# The view rejects bare `len` like `String` (upstream `@unavailable`).
def main():
    var s = String("hello")
    var view = StringSpan(s)
    print(len(view))
