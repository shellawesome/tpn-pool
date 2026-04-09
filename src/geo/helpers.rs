/// Convert ISO 3166-1 alpha-2 country code to country name.
pub fn country_name_from_code(code: &str) -> &str {
    match code.to_uppercase().as_str() {
        "AF" => "Afghanistan", "AL" => "Albania", "DZ" => "Algeria",
        "AR" => "Argentina", "AU" => "Australia", "AT" => "Austria",
        "BE" => "Belgium", "BR" => "Brazil", "CA" => "Canada",
        "CL" => "Chile", "CN" => "China", "CO" => "Colombia",
        "CZ" => "Czech Republic", "DK" => "Denmark", "EG" => "Egypt",
        "FI" => "Finland", "FR" => "France", "DE" => "Germany",
        "GR" => "Greece", "HK" => "Hong Kong", "HU" => "Hungary",
        "IN" => "India", "ID" => "Indonesia", "IE" => "Ireland",
        "IL" => "Israel", "IT" => "Italy", "JP" => "Japan",
        "KR" => "South Korea", "MY" => "Malaysia", "MX" => "Mexico",
        "NL" => "Netherlands", "NZ" => "New Zealand", "NG" => "Nigeria",
        "NO" => "Norway", "PK" => "Pakistan", "PH" => "Philippines",
        "PL" => "Poland", "PT" => "Portugal", "RO" => "Romania",
        "RU" => "Russia", "SA" => "Saudi Arabia", "SG" => "Singapore",
        "ZA" => "South Africa", "ES" => "Spain", "SE" => "Sweden",
        "CH" => "Switzerland", "TW" => "Taiwan", "TH" => "Thailand",
        "TR" => "Turkey", "UA" => "Ukraine", "AE" => "United Arab Emirates",
        "GB" => "United Kingdom", "US" => "United States",
        "VN" => "Vietnam",
        _ => code,
    }
}
