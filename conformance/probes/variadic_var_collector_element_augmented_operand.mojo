def join(var *parts: String) -> String:
    var out = String("")
    for i in range(len(parts)):
        out += parts[i]
    return out


def main():
    print(join("x", "y"))
