# expect: Undefined variable 'x'
# A case binding is scoped to its case body and is gone after the match.
def main():
    match 1:
        case x:
            pass
    print(x)
