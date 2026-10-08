-- Canonical Willie pose/control names for native sampling and client display.
return {
    bones = { "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05", "neck_01", "neck_02", "head",
        "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l", "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
        "thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r" },
    nobody = { spine_01 = true },
    flags = { "R_Guarding", "L_Guarding", "Any_Guarding", "Parrying R", "Parrying L", "R_Thrusting", "L_Thrusting",
        "Alt Thrusting", "Kicking  R", "Kicking  L", "Fallen", "Downed", "R Kneel", "L Kneel", "Is Crouched", "Dodging",
        "R Two Handed Grip", "R Alt Grip", "Mordhau Grip", "Master Stroke Grip", "R Hand Reverse Grip", "L Hand Reverse Grip",
        "Pain Shock", "Being Grabbed", "Grabbed R", "Grabbed L", "Threatening Stance", "R Down", "L Down",
        "Classic Half Swording Toggle", "L Hand In Offhand Attached", "R Kneel Falling" },
    scalars = { "All Body Tonus", "Upper Body Tonus", "Arm R Tonus", "Arm L Tonus", "Leg R Tonus", "Leg L Tonus",
        "Head Tonus", "Muscle Power", "Constraint Rate", "R Thrust Alpha", "Fallen Rate", "Get Up Rate", "Root Linear Constraint Power",
        "Root Angular Constraint Power", "Consciousness", "Stamina" },
    ik = { "R Out End Pos", "L Out End Pos", "R Out Joint Pos", "L Out Joint Pos" },
    grip_r = "R_GripType_Current", grip_l = "L_GripType_Current", aim = "Aim Vector", ctrl_rot = "Current Control Rotation",
    weapon_base = "Root Scene", weapon_tip = "TippyTipScene",
}
