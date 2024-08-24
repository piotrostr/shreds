import json

if __name__ == "__main__":
    want_mints = [
        "3S8qX1MsMqRbiwKg2cQyx7nis1oHMgaCuc9c4VfvVdPN",
        "3B5wuUrMEi5yATD7on46hKfej3pfmd7t1RKgrsN3pump",
        "CTg3ZgYx79zrE1MteDVkmkcGniiFrK1hJ6yiabropump",
        "GiG7Hr61RVm4CSUxJmgiCoySFQtdiwxtqf64MsRppump",
        "EbZh3FDVcgnLNbh1ooatcDL1RCRhBgTKirFKNoGPpump",
        "GYKmdfcUmZVrqfcH1g579BGjuzSRijj3LBuwv79rpump",
        "8Ki8DpuWNxu9VsS3kQbarsCWMcFGWkzzA8pUPto9zBd5",
        "HiHULk2EEF6kGfMar19QywmaTJLUr3LA1em8DyW1pump",
    ]

    with open("./raydium.json", "r") as f:
        data = json.load(f)
    del data["official"]
    data["unOfficial"] = [
        i
        for i in data["unOfficial"]
        if i["baseMint"] in want_mints or i["quoteMint"] in want_mints
    ]

    with open("./raydium.json", "w") as f:
        json.dump(data, f, indent=2)
