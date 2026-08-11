# An omitted lambda return type is fixed to `None`, never inferred from the
# body expression.
# expect: fixed to 'None'
def main():
    var f: def() = lambda: 1 + 1
    f()
