# expect: expression is not a valid match pattern; use '==' to compare against its value
# A mapping pattern is upstream's dictionary literal in pattern position:
# upstream's pattern-matching proposal lists mapping patterns as not yet
# implemented, and the pin rejects the key with this sentence.
def main():
    var d = {"a": 1}
    match d:
        case {"a": x}:
            print(x)
        case _:
            print("other")
