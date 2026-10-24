// GameController v7.0.0-rc.3 config/teams.yaml plus local additions, embedded for portable binaries.
pub(crate) const TEAMS: &[(u8, &str)] = &[
    (0, "Invisibles"),
    (1, "NUbots"),
    (2, "Bold Hearts"),
    (3, "Bembelbots"),
    (4, "Berlin United"),
    (5, "B-Human"),
    (6, "Hamburg Bit-Bots"),
    (7, "Barelang FC"),
    (8, "whIRLwind Amsterdam"),
    (9, "WF Wolves"),
    (10, "Colmillos ITAM"),
    (11, "Rhoban"),
    (12, "Ruhrbot Devils"),
    (13, "Blenders FC"),
    (14, "HTWK Robots"),
    (15, "EWS Bascorro"),
    (16, "Pumas"),
    (17, "ZJUDancer"),
    (18, "rUNSWift"),
    (19, "SPQR Team"),
    (20, "GeoHBots"),
    (21, "RO:BIT"),
    (22, "HUST-HRT"),
    (23, "I-Kid"),
    (24, "HULKs"),
    (25, "HERoEHS"),
    (26, "ICHIRO ITS"),
    (27, "Invic"),
    (28, "Team Sweaty"),
    (29, "CAU Mountain&Sea"),
    (30, "THMOS"),
    (31, "Tsinghua Hephaestus"),
    (32, "UT AustinVilla"),
    (33, "NomadZ"),
    (34, "ITAndroids"),
    (35, "KHUBER"),
    (36, "Mountain"),
    (37, "Team noon"),
    (38, "WolverBot Kickers"),
    (39, "I-Teen"),
    (40, "Inha-United"),
    (41, "RFC-Tsudanuma"),
    (42, "RoboEireann"),
    (43, "Bahia Robotics Team"),
    (44, "Beihang RoboCup Team"),
    (45, "Naova"),
    (46, "BigHeroX"),
    (47, "DUT Future"),
    (48, "Future Becoming"),
    (49, "SABANA HERONS"),
    (50, "R-ZWEI KICKERS"),
    (51, "RedbackBots"),
    (52, "Badger Bots"),
    (53, "KURA"),
    (54, "PCMS-HRG"),
    (55, "Robo-Erectus"),
    (56, "RobôCIn"),
    (57, "Robotedge"),
    (58, "SJTU Cyber Shanhai"),
    (59, "Talos Humanoid Robots"),
    (60, "Tech United"),
    (61, "WarthogRobotics"),
    (62, "Water"),
    (63, "WHU United"),
    (64, "ZETIN"),
    (70, "B-Team"),
    (101, "FC Isar robotics"),
    (102, "foom bots"),
];

pub fn team_number(name: &str) -> Option<u8> {
    let name = name.trim().to_lowercase();
    if name == "htwk" {
        return Some(14);
    }
    TEAMS
        .iter()
        .find(|(_, team)| team.to_lowercase() == name)
        .map(|(number, _)| *number)
}

pub fn team_name(number: u8) -> Option<&'static str> {
    TEAMS
        .iter()
        .find(|(team, _)| *team == number)
        .map(|(_, name)| *name)
}

pub fn display_name(number: u8) -> String {
    team_name(number).map_or_else(|| format!("#{number}"), str::to_owned)
}
